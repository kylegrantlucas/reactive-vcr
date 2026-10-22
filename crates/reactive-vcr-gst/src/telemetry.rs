//! Fire-and-forget UDP telemetry for `stream`: one JSON object per datagram, so a control
//! panel can follow each stream's audio features, modulated values, and health.
//!
//! Sends never block: the socket is non-blocking and a datagram that can't go out is dropped.

use anyhow::{Context as _, Result, anyhow};
use parking_lot::Mutex;
use reactive_vcr::live::LiveProgram;
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Every datagram is smaller than this; a frame message that would not be is skipped.
pub const MAX_DATAGRAM: usize = 8 * 1024;
/// Message format version, sent as `v`.
pub const VERSION: u32 = 1;
/// Span over which `fps` and `proc_ms` are measured.
const WINDOW: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryOptions {
    /// `HOST:PORT` that receives the datagrams.
    pub address: String,
    /// Identifies the stream in every message.
    pub id: String,
    /// Most frame messages per second.
    pub hz: f64,
}

pub(crate) struct Telemetry {
    socket: UdpSocket,
    id: String,
    interval: Duration,
    started: Instant,
    /// Frames the latency-bounding queue discarded.
    dropped: AtomicU64,
    /// The latest hot-reload error, until a reload succeeds.
    mapping_error: Mutex<Option<String>>,
    frames: Mutex<FrameWindow>,
}

#[derive(Default)]
struct FrameWindow {
    /// When each frame of the last `WINDOW` finished, and how long its effect took.
    recent: VecDeque<(Instant, Duration)>,
    /// When the next frame message may go out.
    next_due: Option<Instant>,
}

impl Telemetry {
    pub fn connect(options: &TelemetryOptions) -> Result<Self> {
        let target = options
            .address
            .to_socket_addrs()
            .with_context(|| format!("telemetry address `{}`", options.address))?
            .next()
            .ok_or_else(|| {
                anyhow!(
                    "telemetry address `{}` resolves to nothing",
                    options.address
                )
            })?;
        let local: SocketAddr = if target.is_ipv4() {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        } else {
            (Ipv6Addr::UNSPECIFIED, 0).into()
        };
        let socket = UdpSocket::bind(local).context("opening the telemetry socket")?;
        socket
            .connect(target)
            .with_context(|| format!("telemetry address {target}"))?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            id: options.id.clone(),
            interval: Duration::from_secs_f64(1.0 / options.hz),
            started: Instant::now(),
            dropped: AtomicU64::new(0),
            mapping_error: Mutex::new(None),
            frames: Mutex::new(FrameWindow::default()),
        })
    }

    /// Announce the stream once its pipeline is playing.
    pub fn start(&self, map: &Path, lines: u32) {
        self.send(
            "start",
            json!({ "map": map.display().to_string(), "lines": lines }),
        );
    }

    /// Report the error the stream is about to exit with.
    pub fn error(&self, message: &str) {
        // Leave room for the envelope and for escaping.
        let mut end = message.len().min(MAX_DATAGRAM / 4);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        self.send("error", json!({ "message": &message[..end] }));
    }

    pub fn set_mapping_error(&self, error: Option<String>) {
        *self.mapping_error.lock() = error;
    }

    pub fn frame_dropped(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a processed frame whose effect took `effect`, and at most `hz` times a second
    /// send the stream's state: `frames` processed so far and `live`'s latest features and
    /// target values.
    pub fn frame(&self, effect: Duration, frames: u64, live: &Mutex<LiveProgram>) {
        let now = Instant::now();
        let (fps, proc_ms) = {
            let mut window = self.frames.lock();
            window.recent.push_back((now, effect));
            while window
                .recent
                .front()
                .is_some_and(|&(at, _)| now - at > WINDOW)
            {
                window.recent.pop_front();
            }
            let due = window.next_due.unwrap_or(now);
            if now < due {
                return;
            }
            // Keep to the schedule, so frames that arrive just after each due time still yield
            // `hz` messages a second; after a stall, start over rather than catch up.
            let next = due + self.interval;
            window.next_due = Some(if next > now {
                next
            } else {
                now + self.interval
            });
            let count = window.recent.len();
            // Until a full window has passed, rate over the time since the stream started.
            let span = (now - self.started).clamp(Duration::from_millis(1), WINDOW);
            let total: Duration = window.recent.iter().map(|&(_, effect)| effect).sum();
            (
                count as f64 / span.as_secs_f64(),
                total.as_secs_f64() * 1e3 / count as f64,
            )
        };
        let (features, targets, level_db, gated) = {
            let live = live.lock();
            let current = live.features();
            let bands: Map<String, Value> = live
                .band_names()
                .zip(&current.bands)
                .map(|(name, &value)| (name.to_owned(), rounded(value.into())))
                .collect();
            let targets: Map<String, Value> = live
                .targets()
                .map(|(name, value)| (name.to_owned(), rounded(value)))
                .collect();
            let features = json!({
                "rms": rounded(current.rms.into()),
                "onset": rounded(current.onset.into()),
                "bands": bands,
            });
            // dBFS to a tenth, or `null` at digital silence.
            let level_db = live
                .level_db()
                .map(|level| (f64::from(level) * 10.0).round() / 10.0);
            (features, targets, level_db, live.gated())
        };
        self.send(
            "frame",
            json!({
                "frames": frames,
                "dropped": self.dropped.load(Ordering::Relaxed),
                "fps": rounded(fps),
                "proc_ms": rounded(proc_ms),
                "features": features,
                "targets": targets,
                "mapping_error": self.mapping_error.lock().clone(),
                "level_db": level_db,
                "gated": gated,
            }),
        );
    }

    /// Send `fields` with the common envelope; drop the message if it is too large or the
    /// socket can't take it now.
    fn send(&self, kind: &str, fields: Value) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64);
        let mut message = json!({ "v": VERSION, "id": self.id, "kind": kind, "ts": ts });
        if let (Value::Object(message), Value::Object(fields)) = (&mut message, fields) {
            message.extend(fields);
        }
        let Ok(bytes) = serde_json::to_vec(&message) else {
            return;
        };
        if bytes.len() < MAX_DATAGRAM {
            let _ = self.socket.send(&bytes);
        }
    }
}

/// `value` to four decimal places, which keeps messages short; `null` if not finite.
fn rounded(value: f64) -> Value {
    json!((value * 1e4).round() / 1e4)
}
