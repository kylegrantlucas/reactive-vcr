//! Hand-written libobs declarations, pinned to the OBS Studio 30.0.0 headers
//! (`libobs/obs-source.h`, `obs.h`, `obs-module.h`, `obs-config.h`, `obs-data.h`,
//! `obs-properties.h`, `graphics/graphics.h`, `media-io/audio-io.h`, `util/base.h`).
//! Every layout and signature here must match those headers exactly.
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_void};

macro_rules! opaque {
    ($($name:ident),* $(,)?) => {
        $(
            #[repr(C)]
            pub struct $name {
                _private: [u8; 0],
            }
        )*
    };
}

opaque!(
    obs_source_t,
    obs_weak_source_t,
    obs_data_t,
    obs_properties_t,
    obs_property_t,
    obs_module_t,
    gs_effect_t,
    gs_eparam_t,
    gs_texture_t,
    gs_texrender_t,
    gs_stagesurf_t,
    audio_t,
    video_t,
);

pub const MAX_AV_PLANES: usize = 8;

/// Captured audio: float planar, one plane per output channel.
#[repr(C)]
pub struct audio_data {
    pub data: [*mut u8; MAX_AV_PLANES],
    pub frames: u32,
    pub timestamp: u64,
}

#[repr(C, align(16))]
pub struct vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

/// A callback slot this plugin leaves empty; only its pointer size matters.
pub type Unused = Option<unsafe extern "C" fn()>;

/// `struct obs_source_info` as of OBS 30.0.0, complete: libobs reads `version` from the
/// caller's struct whatever size is passed, so the declaration must reach it. Later OBS
/// versions only append fields, and libobs zero-fills what a smaller struct omits.
#[repr(C)]
pub struct obs_source_info {
    pub id: *const c_char,
    pub type_: c_int,
    pub output_flags: u32,
    pub get_name: Option<unsafe extern "C" fn(type_data: *mut c_void) -> *const c_char>,
    pub create: Option<
        unsafe extern "C" fn(settings: *mut obs_data_t, source: *mut obs_source_t) -> *mut c_void,
    >,
    pub destroy: Option<unsafe extern "C" fn(data: *mut c_void)>,
    pub get_width: Option<unsafe extern "C" fn(data: *mut c_void) -> u32>,
    pub get_height: Option<unsafe extern "C" fn(data: *mut c_void) -> u32>,
    pub get_defaults: Option<unsafe extern "C" fn(settings: *mut obs_data_t)>,
    pub get_properties: Option<unsafe extern "C" fn(data: *mut c_void) -> *mut obs_properties_t>,
    pub update: Option<unsafe extern "C" fn(data: *mut c_void, settings: *mut obs_data_t)>,
    pub activate: Option<unsafe extern "C" fn(data: *mut c_void)>,
    pub deactivate: Option<unsafe extern "C" fn(data: *mut c_void)>,
    pub show: Option<unsafe extern "C" fn(data: *mut c_void)>,
    pub hide: Option<unsafe extern "C" fn(data: *mut c_void)>,
    pub video_tick: Option<unsafe extern "C" fn(data: *mut c_void, seconds: f32)>,
    pub video_render: Option<unsafe extern "C" fn(data: *mut c_void, effect: *mut gs_effect_t)>,
    pub filter_video: Unused,
    pub filter_audio: Unused,
    pub enum_active_sources: Unused,
    pub save: Unused,
    pub load: Unused,
    pub mouse_click: Unused,
    pub mouse_move: Unused,
    pub mouse_wheel: Unused,
    pub focus: Unused,
    pub key_click: Unused,
    pub filter_remove: Unused,
    pub type_data: *mut c_void,
    pub free_type_data: Unused,
    pub audio_render: Unused,
    pub enum_all_sources: Unused,
    pub transition_start: Unused,
    pub transition_stop: Unused,
    pub get_defaults2: Unused,
    pub get_properties2: Unused,
    pub audio_mix: Unused,
    pub icon_type: c_int,
    pub media_play_pause: Unused,
    pub media_restart: Unused,
    pub media_stop: Unused,
    pub media_next: Unused,
    pub media_previous: Unused,
    pub media_get_duration: Unused,
    pub media_get_time: Unused,
    pub media_set_time: Unused,
    pub media_get_state: Unused,
    pub version: u32,
    pub unversioned_id: *const c_char,
    pub missing_files: Unused,
    pub video_get_color_space: Unused,
    pub filter_add: Unused,
}

pub type obs_source_audio_capture_t = unsafe extern "C" fn(
    param: *mut c_void,
    source: *mut obs_source_t,
    audio_data: *const audio_data,
    muted: bool,
);

pub const OBS_SOURCE_TYPE_FILTER: c_int = 1;
pub const OBS_SOURCE_VIDEO: u32 = 1 << 0;
pub const OBS_SOURCE_AUDIO: u32 = 1 << 1;
pub const GS_BGRA: c_int = 5;
pub const GS_ZS_NONE: c_int = 0;
pub const GS_DYNAMIC: u32 = 1 << 1;
pub const GS_CLEAR_COLOR: u32 = 1 << 0;
pub const GS_BLEND_ZERO: c_int = 0;
pub const GS_BLEND_ONE: c_int = 1;
pub const OBS_EFFECT_DEFAULT: c_int = 0;
pub const OBS_PATH_FILE: c_int = 0;
pub const OBS_COMBO_TYPE_LIST: c_int = 2;
pub const OBS_COMBO_FORMAT_STRING: c_int = 3;
pub const OBS_TEXT_INFO: c_int = 3;
pub const LOG_WARNING: c_int = 200;
pub const LOG_INFO: c_int = 300;
pub const LIBOBS_API_VER: u32 = make_semantic_version(30, 0, 0);

/// `MAKE_SEMANTIC_VERSION` from `obs-config.h`.
const fn make_semantic_version(major: u32, minor: u32, patch: u32) -> u32 {
    (major << 24) | (minor << 16) | patch
}

#[cfg_attr(windows, link(name = "obs", kind = "raw-dylib"))]
unsafe extern "C" {
    pub fn obs_register_source_s(info: *const obs_source_info, size: usize);
    pub fn blog(log_level: c_int, format: *const c_char, ...);

    pub fn obs_enter_graphics();
    pub fn obs_leave_graphics();
    pub fn obs_get_base_effect(effect: c_int) -> *mut gs_effect_t;

    pub fn obs_filter_get_target(filter: *const obs_source_t) -> *mut obs_source_t;
    pub fn obs_source_get_base_width(source: *mut obs_source_t) -> u32;
    pub fn obs_source_get_base_height(source: *mut obs_source_t) -> u32;
    pub fn obs_source_video_render(source: *mut obs_source_t);
    pub fn obs_source_skip_video_filter(filter: *mut obs_source_t);

    pub fn obs_enum_sources(
        enum_proc: Option<unsafe extern "C" fn(*mut c_void, *mut obs_source_t) -> bool>,
        param: *mut c_void,
    );
    pub fn obs_source_get_output_flags(source: *const obs_source_t) -> u32;
    pub fn obs_source_get_name(source: *const obs_source_t) -> *const c_char;
    pub fn obs_get_source_by_name(name: *const c_char) -> *mut obs_source_t;
    pub fn obs_source_get_weak_source(source: *mut obs_source_t) -> *mut obs_weak_source_t;
    pub fn obs_weak_source_get_source(weak: *mut obs_weak_source_t) -> *mut obs_source_t;
    pub fn obs_source_release(source: *mut obs_source_t);
    pub fn obs_weak_source_release(weak: *mut obs_weak_source_t);

    pub fn obs_source_add_audio_capture_callback(
        source: *mut obs_source_t,
        callback: obs_source_audio_capture_t,
        param: *mut c_void,
    );
    pub fn obs_source_remove_audio_capture_callback(
        source: *mut obs_source_t,
        callback: obs_source_audio_capture_t,
        param: *mut c_void,
    );

    pub fn obs_get_audio() -> *mut audio_t;
    pub fn obs_get_video() -> *mut video_t;
    pub fn audio_output_get_sample_rate(audio: *const audio_t) -> u32;
    pub fn audio_output_get_channels(audio: *const audio_t) -> usize;
    pub fn video_output_get_frame_rate(video: *const video_t) -> f64;

    pub fn obs_data_get_string(data: *mut obs_data_t, name: *const c_char) -> *const c_char;
    pub fn obs_data_get_int(data: *mut obs_data_t, name: *const c_char) -> i64;
    pub fn obs_data_set_default_string(
        data: *mut obs_data_t,
        name: *const c_char,
        val: *const c_char,
    );
    pub fn obs_data_set_default_int(data: *mut obs_data_t, name: *const c_char, val: i64);

    pub fn obs_properties_create() -> *mut obs_properties_t;
    pub fn obs_properties_add_path(
        props: *mut obs_properties_t,
        name: *const c_char,
        description: *const c_char,
        type_: c_int,
        filter: *const c_char,
        default_path: *const c_char,
    ) -> *mut obs_property_t;
    pub fn obs_properties_add_list(
        props: *mut obs_properties_t,
        name: *const c_char,
        description: *const c_char,
        type_: c_int,
        format: c_int,
    ) -> *mut obs_property_t;
    pub fn obs_property_list_add_string(
        p: *mut obs_property_t,
        name: *const c_char,
        val: *const c_char,
    ) -> usize;
    pub fn obs_properties_add_int_slider(
        props: *mut obs_properties_t,
        name: *const c_char,
        description: *const c_char,
        min: c_int,
        max: c_int,
        step: c_int,
    ) -> *mut obs_property_t;
    pub fn obs_properties_add_text(
        props: *mut obs_properties_t,
        name: *const c_char,
        description: *const c_char,
        type_: c_int,
    ) -> *mut obs_property_t;

    pub fn gs_texrender_create(format: c_int, zsformat: c_int) -> *mut gs_texrender_t;
    pub fn gs_texrender_destroy(texrender: *mut gs_texrender_t);
    pub fn gs_texrender_reset(texrender: *mut gs_texrender_t);
    pub fn gs_texrender_begin(texrender: *mut gs_texrender_t, cx: u32, cy: u32) -> bool;
    pub fn gs_texrender_end(texrender: *mut gs_texrender_t);
    pub fn gs_texrender_get_texture(texrender: *const gs_texrender_t) -> *mut gs_texture_t;
    pub fn gs_stagesurface_create(
        width: u32,
        height: u32,
        color_format: c_int,
    ) -> *mut gs_stagesurf_t;
    pub fn gs_stagesurface_destroy(stagesurf: *mut gs_stagesurf_t);
    pub fn gs_stage_texture(dst: *mut gs_stagesurf_t, src: *mut gs_texture_t);
    pub fn gs_stagesurface_map(
        stagesurf: *mut gs_stagesurf_t,
        data: *mut *mut u8,
        linesize: *mut u32,
    ) -> bool;
    pub fn gs_stagesurface_unmap(stagesurf: *mut gs_stagesurf_t);

    pub fn gs_texture_create(
        width: u32,
        height: u32,
        color_format: c_int,
        levels: u32,
        data: *const *const u8,
        flags: u32,
    ) -> *mut gs_texture_t;
    pub fn gs_texture_destroy(tex: *mut gs_texture_t);
    pub fn gs_texture_set_image(
        tex: *mut gs_texture_t,
        data: *const u8,
        linesize: u32,
        invert: bool,
    );
    pub fn gs_clear(clear_flags: u32, color: *const vec4, depth: f32, stencil: u8);
    pub fn gs_ortho(left: f32, right: f32, top: f32, bottom: f32, znear: f32, zfar: f32);
    pub fn gs_blend_state_push();
    pub fn gs_blend_state_pop();
    pub fn gs_blend_function(src: c_int, dest: c_int);
    pub fn gs_effect_get_param_by_name(
        effect: *const gs_effect_t,
        name: *const c_char,
    ) -> *mut gs_eparam_t;
    pub fn gs_effect_set_texture(param: *mut gs_eparam_t, val: *mut gs_texture_t);
    pub fn gs_effect_loop(effect: *mut gs_effect_t, name: *const c_char) -> bool;
    pub fn gs_draw_sprite(tex: *mut gs_texture_t, flip: u32, width: u32, height: u32);
}
