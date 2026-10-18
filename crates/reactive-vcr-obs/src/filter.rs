//! The `reactive_vcr_filter` video filter.
//!
//! Threads:
//! - `update` and `get_properties` run on the UI thread; `update` only records [`Pending`].
//! - `video_tick` and `video_render` run on the graphics thread and own [`State`].
//! - The audio capture callback runs on the audio thread and touches only [`Shared`].
//! - A worker thread runs the effect on captured frames, so rendering never waits for it.
//!
//! Each tick advances modulation by one frame and schedules one capture; the next render
//! downscales the target into a BGRA readback for the worker and draws the newest processed
//! frame back at the source size.

use crate::ffi::*;
use crate::{guard, log};
use parking_lot::Mutex;
use reactive_vcr::effect::{ChainLayout, EffectSettings, FrameProcessor};
use reactive_vcr::live::LiveProgram;
use reactive_vcr::mapping::CompiledMapping;
use reactive_vcr::params::ParamCatalog;
use reactive_vcr::watch::MappingWatcher;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAPPING: &CStr = c"mapping";
const AUDIO_SOURCE: &CStr = c"audio_source";
const LINES: &CStr = c"lines";
const DEFAULT_LINES: i64 = 480;
const CHOOSE_MAPPING: &str = "Choose a mapping file";
/// How often to look again for an audio source that isn't there yet.
const ATTACH_RETRY: Duration = Duration::from_secs(1);

pub(crate) fn source_info() -> obs_source_info {
    // SAFETY: every field is a nullable pointer, an `Option` of a function pointer, or an
    // integer, so all-zero is a valid "unset" value for each.
    let mut info: obs_source_info = unsafe { std::mem::zeroed() };
    info.id = c"reactive_vcr_filter".as_ptr();
    info.type_ = OBS_SOURCE_TYPE_FILTER;
    info.output_flags = OBS_SOURCE_VIDEO;
    info.get_name = Some(get_name);
    info.create = Some(create);
    info.destroy = Some(destroy);
    info.get_defaults = Some(get_defaults);
    info.get_properties = Some(get_properties);
    info.update = Some(update);
    info.video_tick = Some(video_tick);
    info.video_render = Some(video_render);
    info
}

/// Settings recorded by `update` for the next tick to apply.
struct Pending {
    mapping: String,
    audio_source: String,
    lines: u32,
}

/// State shared with the audio thread.
struct Shared {
    live: Mutex<Option<LiveProgram>>,
    downmix: Mutex<Vec<f32>>,
    status: Mutex<String>,
}

/// A frame for the worker to process.
struct Job {
    frame: usize,
    effect: EffectSettings,
    chain: ChainLayout,
    width: usize,
    height: usize,
    /// Packed BGRA input.
    pixels: Vec<u8>,
    /// Buffer for the packed BGRA output.
    output: Vec<u8>,
}

/// A processed frame, packed BGRA.
struct Done {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

/// The frame that the next render should capture.
struct Capture {
    frame: usize,
    effect: EffectSettings,
    chain: ChainLayout,
}

struct Worker {
    jobs: Option<SyncSender<Job>>,
    recycled: Receiver<Vec<u8>>,
    done: Arc<Mutex<Option<Done>>>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn() -> Self {
        let (jobs, job_rx) = mpsc::sync_channel::<Job>(1);
        let (recycle, recycled) = mpsc::channel();
        let done = Arc::new(Mutex::new(None));
        let published = done.clone();
        let thread = std::thread::Builder::new()
            .name("reactive-vcr-obs".into())
            .spawn(move || work(job_rx, recycle, published))
            .map_err(|error| log(LOG_WARNING, &format!("starting the worker: {error}")))
            .ok();
        Self {
            jobs: thread.as_ref().map(|_| jobs),
            recycled,
            done,
            thread,
        }
    }

    fn stop(&mut self) {
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn work(jobs: Receiver<Job>, recycle: Sender<Vec<u8>>, done: Arc<Mutex<Option<Done>>>) {
    let mut processor: Option<FrameProcessor> = None;
    while let Ok(job) = jobs.recv() {
        let Job {
            frame,
            effect,
            chain,
            width,
            height,
            pixels,
            mut output,
        } = job;
        output.resize(width * height * 4, 0);
        let processed = guard(
            c"processing",
            || false,
            || {
                let processor = match processor.as_mut() {
                    Some(current) if current.fits(width, height, chain) => current,
                    _ => processor.insert(FrameProcessor::new(width, height, chain)),
                };
                processor.process_bgra(&effect, frame, &pixels, width * 4, &mut output, width * 4);
                true
            },
        );
        if processed {
            let replaced = done.lock().replace(Done {
                width,
                height,
                pixels: output,
            });
            if let Some(stale) = replaced {
                let _ = recycle.send(stale.pixels);
            }
        } else {
            processor = None;
            let _ = recycle.send(output);
        }
        let _ = recycle.send(pixels);
    }
}

/// Graphics-thread state.
struct State {
    mapping: String,
    watcher: Option<MappingWatcher>,
    audio_name: String,
    audio: Option<*mut obs_weak_source_t>,
    last_attach: Option<Instant>,
    lines: u32,
    capture: Option<Capture>,
    /// Last status logged, so each change is logged once.
    logged: String,
    texrender: *mut gs_texrender_t,
    stagesurf: *mut gs_stagesurf_t,
    stage_size: (u32, u32),
    texture: *mut gs_texture_t,
    texture_size: (u32, u32),
    pool: Vec<Vec<u8>>,
    worker: Worker,
}

pub(crate) struct Filter {
    source: *mut obs_source_t,
    catalog: Arc<ParamCatalog>,
    shared: Arc<Shared>,
    pending: Mutex<Option<Pending>>,
    state: Mutex<State>,
}

/// Borrow the filter behind OBS's `data` pointer.
///
/// # Safety
/// `data` must be a pointer returned by `create` and not yet passed to `destroy`.
unsafe fn filter<'a>(data: *mut c_void) -> &'a Filter {
    unsafe { &*data.cast::<Filter>() }
}

/// A C string from OBS as an owned `String`; NULL becomes empty.
///
/// # Safety
/// `text` must be NULL or a valid NUL-terminated string.
unsafe fn owned(text: *const c_char) -> String {
    if text.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    }
}

fn even(value: u32) -> u32 {
    (value & !1).max(2)
}

unsafe extern "C" fn get_name(_type_data: *mut c_void) -> *const c_char {
    c"reactive-vcr".as_ptr()
}

unsafe extern "C" fn create(settings: *mut obs_data_t, source: *mut obs_source_t) -> *mut c_void {
    guard(c"create", null_mut, || {
        let filter = Box::new(Filter {
            source,
            catalog: Arc::new(ParamCatalog::new()),
            shared: Arc::new(Shared {
                live: Mutex::new(None),
                downmix: Mutex::new(Vec::new()),
                status: Mutex::new(CHOOSE_MAPPING.into()),
            }),
            pending: Mutex::new(None),
            state: Mutex::new(State {
                mapping: String::new(),
                watcher: None,
                audio_name: String::new(),
                audio: None,
                last_attach: None,
                lines: DEFAULT_LINES as u32,
                capture: None,
                logged: String::new(),
                texrender: null_mut(),
                stagesurf: null_mut(),
                stage_size: (0, 0),
                texture: null_mut(),
                texture_size: (0, 0),
                pool: Vec::new(),
                worker: Worker::spawn(),
            }),
        });
        // SAFETY: OBS passes this source's live settings.
        unsafe { filter.record(settings) };
        Box::into_raw(filter).cast()
    })
}

unsafe extern "C" fn destroy(data: *mut c_void) {
    guard(
        c"destroy",
        || (),
        || {
            // SAFETY: OBS passes the pointer from `create` exactly once to `destroy`.
            let filter = unsafe { Box::from_raw(data.cast::<Filter>()) };
            let mut state = filter.state.lock();
            filter.detach_audio(&mut state);
            state.worker.stop();
            // SAFETY: graphics objects are destroyed inside the graphics context that made them.
            unsafe {
                obs_enter_graphics();
                if !state.texrender.is_null() {
                    gs_texrender_destroy(state.texrender);
                }
                if !state.stagesurf.is_null() {
                    gs_stagesurface_destroy(state.stagesurf);
                }
                if !state.texture.is_null() {
                    gs_texture_destroy(state.texture);
                }
                obs_leave_graphics();
            }
        },
    );
}

unsafe extern "C" fn get_defaults(settings: *mut obs_data_t) {
    guard(
        c"get_defaults",
        || (),
        || {
            // SAFETY: OBS passes a valid settings object; keys and values are NUL-terminated.
            unsafe {
                obs_data_set_default_string(settings, MAPPING.as_ptr(), c"".as_ptr());
                obs_data_set_default_string(settings, AUDIO_SOURCE.as_ptr(), c"".as_ptr());
                obs_data_set_default_int(settings, LINES.as_ptr(), DEFAULT_LINES);
            }
        },
    );
}

unsafe extern "C" fn add_audio_source(param: *mut c_void, source: *mut obs_source_t) -> bool {
    guard(
        c"enum_sources",
        || true,
        || {
            // SAFETY: `param` is the list property passed to `obs_enum_sources`, and OBS passes
            // a live source for the duration of the call.
            unsafe {
                if obs_source_get_output_flags(source) & OBS_SOURCE_AUDIO != 0 {
                    let name = obs_source_get_name(source);
                    if !name.is_null() {
                        obs_property_list_add_string(param.cast(), name, name);
                    }
                }
            }
            true
        },
    )
}

unsafe extern "C" fn get_properties(data: *mut c_void) -> *mut obs_properties_t {
    guard(c"get_properties", null_mut, || {
        let status = if data.is_null() {
            CHOOSE_MAPPING.to_string()
        } else {
            // SAFETY: a non-null `data` is this filter's pointer from `create`.
            unsafe { filter(data) }.shared.status.lock().clone()
        };
        let status = CString::new(status.replace('\0', "")).expect("NULs removed");
        // SAFETY: OBS copies every string these calls receive.
        unsafe {
            let props = obs_properties_create();
            obs_properties_add_path(
                props,
                MAPPING.as_ptr(),
                c"Mapping file".as_ptr(),
                OBS_PATH_FILE,
                c"Mapping (*.toml)".as_ptr(),
                std::ptr::null(),
            );
            let list = obs_properties_add_list(
                props,
                AUDIO_SOURCE.as_ptr(),
                c"Audio source".as_ptr(),
                OBS_COMBO_TYPE_LIST,
                OBS_COMBO_FORMAT_STRING,
            );
            obs_property_list_add_string(list, c"None".as_ptr(), c"".as_ptr());
            obs_enum_sources(Some(add_audio_source), list.cast());
            obs_properties_add_int_slider(
                props,
                LINES.as_ptr(),
                c"Processing lines".as_ptr(),
                120,
                1080,
                2,
            );
            obs_properties_add_text(props, c"status".as_ptr(), status.as_ptr(), OBS_TEXT_INFO);
            props
        }
    })
}

unsafe extern "C" fn update(data: *mut c_void, settings: *mut obs_data_t) {
    guard(
        c"update",
        || (),
        || {
            // SAFETY: OBS passes this filter's pointer and its live settings.
            unsafe { filter(data).record(settings) }
        },
    );
}

unsafe extern "C" fn video_tick(data: *mut c_void, _seconds: f32) {
    guard(
        c"video_tick",
        || (),
        || {
            // SAFETY: OBS passes this filter's pointer.
            unsafe { filter(data) }.tick()
        },
    );
}

unsafe extern "C" fn video_render(data: *mut c_void, _effect: *mut gs_effect_t) {
    // SAFETY: OBS passes this filter's pointer.
    let filter = unsafe { filter(data) };
    guard(
        c"video_render",
        // SAFETY: rendering the target unfiltered is always a valid fallback.
        || unsafe { obs_source_skip_video_filter(filter.source) },
        || filter.render(),
    );
}

/// Downmix captured audio and feed it to the live program.
unsafe extern "C" fn capture_audio(
    param: *mut c_void,
    _source: *mut obs_source_t,
    audio: *const audio_data,
    muted: bool,
) {
    guard(
        c"audio_capture",
        || (),
        || {
            // SAFETY: `param` is the `Shared` the filter keeps alive until this callback is
            // removed, and OBS passes valid audio for the duration of the call.
            let (shared, audio) = unsafe { (&*param.cast::<Shared>(), &*audio) };
            let frames = audio.frames as usize;
            let mut downmix = shared.downmix.lock();
            downmix.clear();
            downmix.resize(frames, 0.0);
            if !muted {
                // SAFETY: libobs owns the audio output for the whole session.
                let channels =
                    unsafe { audio_output_get_channels(obs_get_audio()) }.min(MAX_AV_PLANES);
                let mut used = 0;
                for plane in &audio.data[..channels] {
                    if plane.is_null() {
                        continue;
                    }
                    // SAFETY: OBS delivers float planar audio with `frames` samples per plane.
                    let samples =
                        unsafe { std::slice::from_raw_parts(plane.cast::<f32>(), frames) };
                    for (mixed, sample) in downmix.iter_mut().zip(samples) {
                        *mixed += sample;
                    }
                    used += 1;
                }
                if used > 1 {
                    let scale = 1.0 / used as f32;
                    downmix.iter_mut().for_each(|sample| *sample *= scale);
                }
            }
            if let Some(live) = shared.live.lock().as_mut() {
                live.push_audio(&downmix);
            }
        },
    );
}

impl Filter {
    /// Record settings for the next tick.
    ///
    /// # Safety
    /// `settings` must be a valid OBS settings object.
    unsafe fn record(&self, settings: *mut obs_data_t) {
        let (mapping, audio_source, lines) = unsafe {
            (
                owned(obs_data_get_string(settings, MAPPING.as_ptr())),
                owned(obs_data_get_string(settings, AUDIO_SOURCE.as_ptr())),
                obs_data_get_int(settings, LINES.as_ptr()),
            )
        };
        *self.pending.lock() = Some(Pending {
            mapping,
            audio_source,
            lines: lines.clamp(2, 2160) as u32,
        });
    }

    fn set_status(&self, state: &mut State, status: String, error: bool) {
        if error && status != state.logged {
            log(LOG_WARNING, &status);
        } else if !error && status != state.logged {
            log(LOG_INFO, &status);
        }
        state.logged.clone_from(&status);
        *self.shared.status.lock() = status;
    }

    /// Install a freshly loaded mapping, keeping the running program's audio.
    fn install(&self, map: CompiledMapping) {
        let map = Arc::new(map);
        let mut live = self.shared.live.lock();
        match live.as_mut() {
            Some(program) => program.set_mapping(map),
            None => {
                // SAFETY: libobs owns the audio and video outputs for the whole session.
                let (sample_rate, fps) = unsafe {
                    (
                        audio_output_get_sample_rate(obs_get_audio()),
                        video_output_get_frame_rate(obs_get_video()),
                    )
                };
                *live = Some(LiveProgram::new(
                    self.catalog.clone(),
                    map,
                    sample_rate,
                    fps,
                ));
            }
        }
    }

    fn load_result(&self, state: &mut State, loaded: anyhow::Result<CompiledMapping>) {
        match loaded {
            Ok(map) => {
                self.install(map);
                let status = format!("Loaded {}", state.mapping);
                self.set_status(state, status, false);
            }
            Err(error) => {
                let kept = if self.shared.live.lock().is_some() {
                    "; keeping the previous mapping"
                } else {
                    ""
                };
                self.set_status(state, format!("Mapping error: {error:#}{kept}"), true);
            }
        }
    }

    fn tick(&self) {
        let mut state = self.state.lock();
        let state = &mut *state;
        if let Some(pending) = self.pending.lock().take() {
            if pending.mapping != state.mapping {
                state.mapping = pending.mapping;
                if state.mapping.is_empty() {
                    state.watcher = None;
                    *self.shared.live.lock() = None;
                    self.set_status(state, CHOOSE_MAPPING.into(), false);
                } else {
                    let mut watcher = MappingWatcher::new(state.mapping.clone().into());
                    let loaded = watcher.load(&self.catalog);
                    state.watcher = Some(watcher);
                    self.load_result(state, loaded);
                }
            }
            if pending.audio_source != state.audio_name {
                self.detach_audio(state);
                state.audio_name = pending.audio_source;
                state.last_attach = None;
            }
            state.lines = pending.lines;
        }

        if let Some(loaded) = state
            .watcher
            .as_mut()
            .and_then(|watcher| watcher.poll(&self.catalog))
        {
            self.load_result(state, loaded);
        }

        if !state.audio_name.is_empty()
            && state
                .last_attach
                .is_none_or(|last| last.elapsed() >= ATTACH_RETRY)
        {
            state.last_attach = Some(Instant::now());
            if !self.audio_alive(state) {
                self.detach_audio(state);
                self.attach_audio(state);
            }
        }

        let mut live = self.shared.live.lock();
        if let Some(program) = live.as_mut() {
            // SAFETY: libobs owns the video output for the whole session.
            program.set_fps(unsafe { video_output_get_frame_rate(obs_get_video()) });
            let (frame, effect) = program.next_frame();
            state.capture = Some(Capture {
                frame,
                effect,
                chain: program.chain(),
            });
        } else {
            state.capture = None;
        }
    }

    /// Whether the attached audio source still exists.
    fn audio_alive(&self, state: &State) -> bool {
        let Some(weak) = state.audio else {
            return false;
        };
        // SAFETY: `weak` is a reference this filter holds.
        unsafe {
            let source = obs_weak_source_get_source(weak);
            if source.is_null() {
                return false;
            }
            obs_source_release(source);
        }
        true
    }

    fn attach_audio(&self, state: &mut State) {
        let Ok(name) = CString::new(state.audio_name.as_str()) else {
            return;
        };
        // SAFETY: `name` is NUL-terminated; the strong reference is released after taking a
        // weak one, and `Shared` outlives the callback, which `detach_audio` removes.
        unsafe {
            let source = obs_get_source_by_name(name.as_ptr());
            if source.is_null() {
                return;
            }
            obs_source_add_audio_capture_callback(
                source,
                capture_audio,
                Arc::as_ptr(&self.shared).cast_mut().cast(),
            );
            state.audio = Some(obs_source_get_weak_source(source));
            obs_source_release(source);
        }
    }

    fn detach_audio(&self, state: &mut State) {
        let Some(weak) = state.audio.take() else {
            return;
        };
        // SAFETY: `weak` is a reference this filter holds. libobs removes the callback under
        // its callback lock, so no call is in flight once this returns.
        unsafe {
            let source = obs_weak_source_get_source(weak);
            if !source.is_null() {
                obs_source_remove_audio_capture_callback(
                    source,
                    capture_audio,
                    Arc::as_ptr(&self.shared).cast_mut().cast(),
                );
                obs_source_release(source);
            }
            obs_weak_source_release(weak);
        }
    }

    fn render(&self) {
        let mut state = self.state.lock();
        let state = &mut *state;
        // SAFETY: called from `video_render`, on the graphics thread inside its context.
        unsafe {
            let target = obs_filter_get_target(self.source);
            if target.is_null() || self.shared.live.lock().is_none() {
                obs_source_skip_video_filter(self.source);
                return;
            }
            let (width, height) = (
                obs_source_get_base_width(target),
                obs_source_get_base_height(target),
            );
            if width == 0 || height == 0 {
                obs_source_skip_video_filter(self.source);
                return;
            }
            if let Some(capture) = state.capture.take() {
                let lines = even(state.lines.min(height));
                let columns = even((width as f64 * lines as f64 / height as f64).round() as u32);
                self.capture(state, target, (width, height), (columns, lines), capture);
            }
            self.upload(state);
            if state.texture.is_null() {
                obs_source_skip_video_filter(self.source);
                return;
            }
            let effect = obs_get_base_effect(OBS_EFFECT_DEFAULT);
            gs_effect_set_texture(
                gs_effect_get_param_by_name(effect, c"image".as_ptr()),
                state.texture,
            );
            while gs_effect_loop(effect, c"Draw".as_ptr()) {
                gs_draw_sprite(state.texture, 0, width, height);
            }
        }
    }

    /// Render `target` at `size` into a readback and hand it to the worker. Drops the frame
    /// if the worker is still busy.
    ///
    /// # Safety
    /// Must run on the graphics thread inside its context, with `target` live.
    unsafe fn capture(
        &self,
        state: &mut State,
        target: *mut obs_source_t,
        (source_width, source_height): (u32, u32),
        (width, height): (u32, u32),
        capture: Capture,
    ) {
        let Some(jobs) = state.worker.jobs.clone() else {
            return;
        };
        unsafe {
            if state.texrender.is_null() {
                state.texrender = gs_texrender_create(GS_BGRA, GS_ZS_NONE);
            }
            gs_texrender_reset(state.texrender);
            if !gs_texrender_begin(state.texrender, width, height) {
                return;
            }
            let clear = vec4 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 0.0,
            };
            gs_clear(GS_CLEAR_COLOR, &clear, 0.0, 0);
            gs_ortho(
                0.0,
                source_width as f32,
                0.0,
                source_height as f32,
                -100.0,
                100.0,
            );
            gs_blend_state_push();
            gs_blend_function(GS_BLEND_ONE, GS_BLEND_ZERO);
            obs_source_video_render(target);
            gs_blend_state_pop();
            gs_texrender_end(state.texrender);

            if state.stage_size != (width, height) || state.stagesurf.is_null() {
                if !state.stagesurf.is_null() {
                    gs_stagesurface_destroy(state.stagesurf);
                }
                state.stagesurf = gs_stagesurface_create(width, height, GS_BGRA);
                state.stage_size = (width, height);
            }
            gs_stage_texture(state.stagesurf, gs_texrender_get_texture(state.texrender));
            let mut data = null_mut();
            let mut linesize = 0;
            if !gs_stagesurface_map(state.stagesurf, &mut data, &mut linesize) {
                return;
            }
            let row = width as usize * 4;
            let rows = height as usize;
            let mut pixels = self.buffer(state);
            pixels.resize(row * rows, 0);
            for (y, packed) in pixels.chunks_exact_mut(row).enumerate() {
                let line = std::slice::from_raw_parts(data.add(y * linesize as usize), row);
                packed.copy_from_slice(line);
            }
            gs_stagesurface_unmap(state.stagesurf);

            let job = Job {
                frame: capture.frame,
                effect: capture.effect,
                chain: capture.chain,
                width: width as usize,
                height: rows,
                pixels,
                output: self.buffer(state),
            };
            match jobs.try_send(job) {
                Ok(()) => {}
                Err(TrySendError::Full(job) | TrySendError::Disconnected(job)) => {
                    state.pool.push(job.pixels);
                    state.pool.push(job.output);
                }
            }
        }
    }

    /// A reusable frame buffer, collecting any the worker has returned.
    fn buffer(&self, state: &mut State) -> Vec<u8> {
        state.pool.extend(state.worker.recycled.try_iter());
        state.pool.pop().unwrap_or_default()
    }

    /// Upload the newest processed frame, if any.
    ///
    /// # Safety
    /// Must run on the graphics thread inside its context.
    unsafe fn upload(&self, state: &mut State) {
        let Some(done) = state.worker.done.lock().take() else {
            return;
        };
        let size = (done.width as u32, done.height as u32);
        unsafe {
            if state.texture_size != size || state.texture.is_null() {
                if !state.texture.is_null() {
                    gs_texture_destroy(state.texture);
                }
                state.texture =
                    gs_texture_create(size.0, size.1, GS_BGRA, 1, std::ptr::null(), GS_DYNAMIC);
                state.texture_size = size;
            }
            if !state.texture.is_null() {
                gs_texture_set_image(state.texture, done.pixels.as_ptr(), size.0 * 4, false);
            }
        }
        state.pool.push(done.pixels);
    }
}
