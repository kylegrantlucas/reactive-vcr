use ntsc_rs::NtscEffect;
use ntsc_rs::settings::UseField;
use ntsc_rs::yiq_fielding::{BlitInfo, DeinterlaceMode, Rgb, YiqView, pixel_bytes_for};

/// Where the bender-rs bends run relative to ntsc-rs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BendOrder {
    #[default]
    Before,
    After,
}

/// Static stage layout from the mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainLayout {
    pub ntsc: bool,
    pub bend_order: BendOrder,
}

impl Default for ChainLayout {
    fn default() -> Self {
        Self {
            ntsc: true,
            bend_order: BendOrder::Before,
        }
    }
}

/// Per-frame settings for every effect engine.
#[derive(Debug, Clone, Default)]
pub struct EffectSettings {
    pub ntsc: NtscEffect,
    pub bender: bender::BendEffect,
}

/// Applies the ntsc-rs and bender-rs stages to tightly packed RGB24 frames, reusing scratch buffers.
pub struct FrameProcessor {
    width: usize,
    height: usize,
    chain: ChainLayout,
    /// ntsc-rs YIQ scratch.
    scratch: Vec<f32>,
    bend_state: bender::BendState,
    rgba_in: Vec<u8>,
    rgba_out: Vec<u8>,
    /// RGB24 frame between the two stages.
    staged: Vec<u8>,
}

impl FrameProcessor {
    pub fn new(width: usize, height: usize, chain: ChainLayout) -> Self {
        let scratch = if chain.ntsc {
            vec![0.0; YiqView::max_buf_length_for((width, height), UseField::Both)]
        } else {
            Vec::new()
        };
        Self {
            width,
            height,
            chain,
            scratch,
            bend_state: bender::BendState::default(),
            rgba_in: Vec::new(),
            rgba_out: Vec::new(),
            staged: Vec::new(),
        }
    }

    pub fn frame_bytes(&self) -> usize {
        self.width * self.height * pixel_bytes_for::<Rgb, u8>()
    }

    pub fn process(
        &mut self,
        effects: &EffectSettings,
        frame_num: usize,
        input: &[u8],
        output: &mut [u8],
    ) {
        assert_eq!(input.len(), self.frame_bytes(), "input frame size");
        assert_eq!(output.len(), self.frame_bytes(), "output frame size");

        let bend = bends_enabled(&effects.bender);
        match (self.chain.ntsc, bend) {
            (false, false) => output.copy_from_slice(input),
            (true, false) => self.apply_ntsc(&effects.ntsc, frame_num, input, output),
            (false, true) => self.apply_bend(&effects.bender, frame_num, input, output),
            (true, true) => {
                let mut staged = std::mem::take(&mut self.staged);
                staged.resize(self.frame_bytes(), 0);
                match self.chain.bend_order {
                    BendOrder::Before => {
                        self.apply_bend(&effects.bender, frame_num, input, &mut staged);
                        self.apply_ntsc(&effects.ntsc, frame_num, &staged, output);
                    }
                    BendOrder::After => {
                        self.apply_ntsc(&effects.ntsc, frame_num, input, &mut staged);
                        self.apply_bend(&effects.bender, frame_num, &staged, output);
                    }
                }
                self.staged = staged;
            }
        }
    }

    fn apply_ntsc(
        &mut self,
        effect: &NtscEffect,
        frame_num: usize,
        input: &[u8],
        output: &mut [u8],
    ) {
        let ctx = ntsc_rs::ctx::global();
        let dimensions = (self.width, self.height);
        let field = effect.use_field.to_yiq_field(frame_num);
        let len = YiqView::buf_length_for(dimensions, field);
        let mut view = YiqView::from_parts(&mut self.scratch[..len], dimensions, field);
        let row_bytes = self.width * pixel_bytes_for::<Rgb, u8>();
        let blit = BlitInfo::from_full_frame(self.width, self.height, row_bytes);
        view.set_from_strided_buffer::<Rgb, u8, _>(ctx, input, blit, ());
        effect.apply_effect_to_yiq(ctx, &mut view, frame_num, [1.0, 1.0]);
        view.write_to_strided_buffer::<Rgb, u8, _>(ctx, output, blit, DeinterlaceMode::Bob, ());
    }

    fn apply_bend(
        &mut self,
        effect: &bender::BendEffect,
        frame_num: usize,
        input_rgb: &[u8],
        output_rgb: &mut [u8],
    ) {
        let (w, h) = (self.width, self.height);
        let rgba_len = w * h * 4;
        self.rgba_in.resize(rgba_len, 0);
        self.rgba_out.resize(rgba_len, 0);
        let (rgb_in, _) = input_rgb.as_chunks::<3>();
        let (rgba_in, _) = self.rgba_in.as_chunks_mut::<4>();
        for (&[r, g, b], rgba) in rgb_in.iter().zip(rgba_in) {
            *rgba = [r, g, b, 255];
        }
        effect.apply_rgba8(
            w,
            h,
            &self.rgba_in,
            w * 4,
            &mut self.rgba_out,
            w * 4,
            bender::PixelOrder::Rgba,
            frame_num as u64,
            &mut self.bend_state,
        );
        let (rgba_out, _) = self.rgba_out.as_chunks::<4>();
        let (rgb_out, _) = output_rgb.as_chunks_mut::<3>();
        for (&[r, g, b, _], rgb) in rgba_out.iter().zip(rgb_out) {
            *rgb = [r, g, b];
        }
    }
}

/// True when any bender stage is switched into the chain.
fn bends_enabled(effect: &bender::BendEffect) -> bool {
    bender::StageId::ALL
        .into_iter()
        .any(|stage| effect.stage_enabled(stage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{ParamCatalog, set_param};

    fn test_frame(w: usize, h: usize) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                v[i] = (x * 255 / w) as u8;
                v[i + 1] = (y * 255 / h) as u8;
                v[i + 2] = if (x / 8 + y / 8) % 2 == 0 { 255 } else { 0 };
            }
        }
        v
    }

    fn inverted() -> EffectSettings {
        let mut e = EffectSettings::default();
        e.bender.invert.enabled = true;
        e
    }

    fn run(chain: ChainLayout, e: &EffectSettings, input: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut fp = FrameProcessor::new(w, h, chain);
        let mut out = vec![0u8; fp.frame_bytes()];
        fp.process(e, 0, input, &mut out);
        out
    }

    #[test]
    fn effect_changes_pixels_and_is_deterministic() {
        let (w, h) = (128, 72);
        let input = test_frame(w, h);
        let mut fp = FrameProcessor::new(w, h, ChainLayout::default());
        assert_eq!(fp.frame_bytes(), w * h * 3);
        let mut a = vec![0u8; w * h * 3];
        let mut b = vec![0u8; w * h * 3];
        let e = EffectSettings::default();
        fp.process(&e, 0, &input, &mut a);
        fp.process(&e, 0, &input, &mut b);
        assert_eq!(a, b, "same effect + frame_num must be deterministic");
        assert_ne!(a, input, "effect should alter the image");
    }

    #[test]
    fn parameter_change_alters_output() {
        let (w, h) = (128, 72);
        let input = test_frame(w, h);
        let mut fp = FrameProcessor::new(w, h, ChainLayout::default());
        let c = ParamCatalog::new();
        let mut loud = EffectSettings::default();
        set_param(&mut loud, c.get("snow_intensity").unwrap(), 50.0).unwrap();
        let mut a = vec![0u8; w * h * 3];
        let mut b = vec![0u8; w * h * 3];
        fp.process(&EffectSettings::default(), 3, &input, &mut a);
        fp.process(&loud, 3, &input, &mut b);
        assert_ne!(a, b);
    }

    #[test]
    fn every_field_mode_works_when_switching_on_reused_processor() {
        let (w, h) = (32, 18);
        let input = test_frame(w, h);
        let mut fp = FrameProcessor::new(w, h, ChainLayout::default());
        let catalog = ParamCatalog::new();
        let field = catalog.get("use_field").unwrap();
        for mode in 0..6 {
            let mut effect = EffectSettings::default();
            set_param(&mut effect, field, mode as f64).unwrap();
            let mut output = vec![0u8; fp.frame_bytes()];
            fp.process(&effect, mode, &input, &mut output);
            assert_eq!(output.len(), w * h * 3);
        }
    }

    #[test]
    fn bypassed_chain_without_bends_passes_frames_through() {
        let (w, h) = (32, 18);
        let input = test_frame(w, h);
        let chain = ChainLayout {
            ntsc: false,
            ..ChainLayout::default()
        };
        assert_eq!(run(chain, &EffectSettings::default(), &input, w, h), input);
    }

    #[test]
    fn bender_stage_alters_frames_and_is_deterministic() {
        let (w, h) = (64, 36);
        let input = test_frame(w, h);
        let chain = ChainLayout {
            ntsc: false,
            ..ChainLayout::default()
        };
        let e = inverted();
        let a = run(chain, &e, &input, w, h);
        let b = run(chain, &e, &input, w, h);
        assert_ne!(a, input, "solarize should alter the image");
        assert_eq!(a, b, "fresh processors must agree");
    }

    #[test]
    fn bend_order_changes_result() {
        let (w, h) = (64, 36);
        let input = test_frame(w, h);
        let e = inverted();
        let before = run(ChainLayout::default(), &e, &input, w, h);
        let after = run(
            ChainLayout {
                ntsc: true,
                bend_order: BendOrder::After,
            },
            &e,
            &input,
            w,
            h,
        );
        assert_ne!(before, after);
    }
}
