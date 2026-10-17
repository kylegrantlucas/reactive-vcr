//! Background work: probing and decoding inputs, and rebuilding the effect program when the
//! mapping changes, without blocking the UI thread.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use eframe::egui;
use reactive_vcr::audio::features::FeatureTimeline;
use reactive_vcr::mapping::{CompiledMapping, Source};
use reactive_vcr::params::ParamCatalog;
use reactive_vcr::timeline::{FeatureCache, Selection};
use reactive_vcr_gst::Program;

/// A value produced once on a worker thread.
pub enum Task<T> {
    Running(Receiver<Result<T, String>>),
    Ready(T),
    Failed(String),
}

impl<T: Send + 'static> Task<T> {
    /// Runs `work` on a new thread and repaints `ctx` when it finishes.
    pub fn spawn(
        ctx: &egui::Context,
        work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work().map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        Self::Running(rx)
    }
}

impl<T> Task<T> {
    /// Collects a finished result. Returns true when the task just finished.
    pub fn poll(&mut self) -> bool {
        let Self::Running(rx) = self else {
            return false;
        };
        *self = match rx.try_recv() {
            Ok(Ok(value)) => Self::Ready(value),
            Ok(Err(error)) => Self::Failed(error),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Self::Failed("the worker thread crashed".into()),
        };
        true
    }

    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running(_))
    }
}

/// One modulation as the meters show it.
pub struct MeterTarget {
    pub param: usize,
    pub source: String,
    /// Output range; bool targets use `0..1`.
    pub range: (f32, f32),
}

/// A built program plus the features and mapping facts the meters read.
pub struct Analysis {
    pub program: Arc<Program>,
    pub features: Arc<FeatureTimeline>,
    pub bands: Vec<String>,
    pub targets: Vec<MeterTarget>,
}

type Request = (u64, CompiledMapping);
type Reply = (u64, Result<Analysis, String>);

/// Rebuilds programs on a thread from the decoded audio. Requests that arrive while a build
/// runs collapse into the newest one; features are re-extracted only when bands,
/// normalization, or onset sensitivity change.
pub struct ProgramWorker {
    requests: Sender<Request>,
    replies: Receiver<Reply>,
}

impl ProgramWorker {
    pub fn new(
        ctx: &egui::Context,
        catalog: Arc<ParamCatalog>,
        samples: Arc<Vec<f32>>,
        selection: Selection,
    ) -> Self {
        let (requests, request_rx) = mpsc::channel::<Request>();
        let (reply_tx, replies) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut cache = FeatureCache::default();
            while let Ok(mut request) = request_rx.recv() {
                while let Ok(newer) = request_rx.try_recv() {
                    request = newer;
                }
                let (generation, map) = request;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    analyze(&mut cache, &catalog, &samples, &selection, &map)
                }))
                .unwrap_or_else(|_| Err("building the effect program panicked".into()));
                if reply_tx.send((generation, result)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        Self { requests, replies }
    }

    pub fn request(&self, generation: u64, map: CompiledMapping) {
        let _ = self.requests.send((generation, map));
    }

    /// The newest finished build, if any.
    pub fn poll(&self) -> Option<Reply> {
        self.replies.try_iter().last()
    }
}

fn analyze(
    cache: &mut FeatureCache,
    catalog: &Arc<ParamCatalog>,
    samples: &[f32],
    selection: &Selection,
    map: &CompiledMapping,
) -> Result<Analysis, String> {
    let features = cache
        .features(samples, map, selection)
        .map_err(|e| format!("analyzing audio: {e:#}"))?;
    let targets = map
        .modulations
        .iter()
        .map(|m| MeterTarget {
            param: m.param,
            source: match m.source {
                Source::Rms => "rms".into(),
                Source::Onset => "onset".into(),
                Source::Band(i) => format!("band:{}", map.bands[i].0),
            },
            range: m.range,
        })
        .collect();
    Ok(Analysis {
        program: Arc::new(Program::new(catalog.clone(), map, &features, selection)),
        features,
        bands: map.bands.iter().map(|(name, _)| name.clone()).collect(),
        targets,
    })
}
