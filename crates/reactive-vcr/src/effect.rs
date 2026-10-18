use ntsc_rs::NtscEffect;
use ntsc_rs::settings::UseField;
use ntsc_rs::yiq_fielding::{BlitInfo, DeinterlaceMode, Rgb, YiqView, pixel_bytes_for};

/// Where the bender-rs bends run relative to ntsc-rs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
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
    /// Packed RGB24 input and output for `process_bgra`.
    bgra_rgb_in: Vec<u8>,
    bgra_rgb_out: Vec<u8>,
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
            bgra_rgb_in: Vec::new(),
            bgra_rgb_out: Vec::new(),
        }
    }

    pub fn frame_bytes(&self) -> usize {
        self.height * self.row_bytes()
    }

    fn row_bytes(&self) -> usize {
        self.width * pixel_bytes_for::<Rgb, u8>()
    }

    /// Whether this processor was built for frames of this size and stage layout.
    pub fn fits(&self, width: usize, height: usize, chain: ChainLayout) -> bool {
        (self.width, self.height, self.chain) == (width, height, chain)
    }

    /// Process tightly packed RGB24 frames.
    pub fn process(
        &mut self,
        effects: &EffectSettings,
        frame_num: usize,
        input: &[u8],
        output: &mut [u8],
    ) {
        assert_eq!(input.len(), self.frame_bytes(), "input frame size");
        assert_eq!(output.len(), self.frame_bytes(), "output frame size");
        let row_bytes = self.row_bytes();
        self.process_strided(effects, frame_num, input, row_bytes, output, row_bytes);
    }

    /// Process RGB24 frames whose rows start every `*_stride` bytes.
    pub fn process_strided(
        &mut self,
        effects: &EffectSettings,
        frame_num: usize,
        input: &[u8],
        input_stride: usize,
        output: &mut [u8],
        output_stride: usize,
    ) {
        let row_bytes = self.row_bytes();
        for (name, len, stride) in [
            ("input", input.len(), input_stride),
            ("output", output.len(), output_stride),
        ] {
            assert!(stride >= row_bytes, "{name} stride shorter than a row");
            assert!(len >= self.height * stride, "{name} frame size");
        }
        let input = Strided::new(input, input_stride);

        let bend = bends_enabled(&effects.bender);
        match (self.chain.ntsc, bend) {
            (false, false) => {
                for y in 0..self.height {
                    let (src, dst) = (y * input_stride, y * output_stride);
                    output[dst..dst + row_bytes].copy_from_slice(&input.data[src..src + row_bytes]);
                }
            }
            (true, false) => {
                self.apply_ntsc(&effects.ntsc, frame_num, input, output, output_stride)
            }
            (false, true) => {
                self.apply_bend(&effects.bender, frame_num, input, output, output_stride)
            }
            (true, true) => {
                let mut staged = std::mem::take(&mut self.staged);
                staged.resize(self.frame_bytes(), 0);
                match self.chain.bend_order {
                    BendOrder::Before => {
                        self.apply_bend(&effects.bender, frame_num, input, &mut staged, row_bytes);
                        let staged_in = Strided::new(&staged, row_bytes);
                        self.apply_ntsc(&effects.ntsc, frame_num, staged_in, output, output_stride);
                    }
                    BendOrder::After => {
                        self.apply_ntsc(&effects.ntsc, frame_num, input, &mut staged, row_bytes);
                        let staged_in = Strided::new(&staged, row_bytes);
                        self.apply_bend(
                            &effects.bender,
                            frame_num,
                            staged_in,
                            output,
                            output_stride,
                        );
                    }
                }
                self.staged = staged;
            }
        }
    }

    /// Process BGRA frames whose rows start every `*_stride` bytes. Each output pixel keeps its
    /// input alpha.
    pub fn process_bgra(
        &mut self,
        effects: &EffectSettings,
        frame_num: usize,
        input: &[u8],
        input_stride: usize,
        output: &mut [u8],
        output_stride: usize,
    ) {
        let bgra_row = self.width * 4;
        for (name, len, stride) in [
            ("input", input.len(), input_stride),
            ("output", output.len(), output_stride),
        ] {
            assert!(stride >= bgra_row, "{name} stride shorter than a row");
            assert!(len >= self.height * stride, "{name} frame size");
        }
        let row_bytes = self.row_bytes();
        let mut rgb_in = std::mem::take(&mut self.bgra_rgb_in);
        let mut rgb_out = std::mem::take(&mut self.bgra_rgb_out);
        rgb_in.resize(self.frame_bytes(), 0);
        rgb_out.resize(self.frame_bytes(), 0);
        for (y, rgb_row) in rgb_in.chunks_exact_mut(row_bytes).enumerate() {
            let start = y * input_stride;
            let (bgra, _) = input[start..start + bgra_row].as_chunks::<4>();
            let (rgb, _) = rgb_row.as_chunks_mut::<3>();
            for (&[b, g, r, _], rgb) in bgra.iter().zip(rgb) {
                *rgb = [r, g, b];
            }
        }
        self.process(effects, frame_num, &rgb_in, &mut rgb_out);
        for (y, rgb_row) in rgb_out.chunks_exact(row_bytes).enumerate() {
            let (in_start, out_start) = (y * input_stride, y * output_stride);
            let (alpha, _) = input[in_start..in_start + bgra_row].as_chunks::<4>();
            let (bgra, _) = output[out_start..out_start + bgra_row].as_chunks_mut::<4>();
            let (rgb, _) = rgb_row.as_chunks::<3>();
            for ((&[r, g, b], source), bgra) in rgb.iter().zip(alpha).zip(bgra) {
                *bgra = [b, g, r, source[3]];
            }
        }
        self.bgra_rgb_in = rgb_in;
        self.bgra_rgb_out = rgb_out;
    }

    fn apply_ntsc(
        &mut self,
        effect: &NtscEffect,
        frame_num: usize,
        input: Strided<'_>,
        output: &mut [u8],
        output_stride: usize,
    ) {
        let ctx = ntsc_rs::ctx::global();
        let dimensions = (self.width, self.height);
        let field = effect.use_field.to_yiq_field(frame_num);
        let len = YiqView::buf_length_for(dimensions, field);
        let mut view = YiqView::from_parts(&mut self.scratch[..len], dimensions, field);
        let read = BlitInfo::from_full_frame(self.width, self.height, input.stride);
        view.set_from_strided_buffer::<Rgb, u8, _>(ctx, input.data, read, ());
        effect.apply_effect_to_yiq(ctx, &mut view, frame_num, [1.0, 1.0]);
        let write = BlitInfo::from_full_frame(self.width, self.height, output_stride);
        view.write_to_strided_buffer::<Rgb, u8, _>(ctx, output, write, DeinterlaceMode::Bob, ());
    }

    fn apply_bend(
        &mut self,
        effect: &bender::BendEffect,
        frame_num: usize,
        input: Strided<'_>,
        output_rgb: &mut [u8],
        output_stride: usize,
    ) {
        let (w, h) = (self.width, self.height);
        let rgba_len = w * h * 4;
        self.rgba_in.resize(rgba_len, 0);
        self.rgba_out.resize(rgba_len, 0);
        let row_bytes = self.row_bytes();
        for (y, rgba_row) in self.rgba_in.chunks_exact_mut(w * 4).enumerate() {
            let start = y * input.stride;
            let (rgb_row, _) = input.data[start..start + row_bytes].as_chunks::<3>();
            let (rgba_row, _) = rgba_row.as_chunks_mut::<4>();
            for (&[r, g, b], rgba) in rgb_row.iter().zip(rgba_row) {
                *rgba = [r, g, b, 255];
            }
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
        for (y, rgba_row) in self.rgba_out.chunks_exact(w * 4).enumerate() {
            let start = y * output_stride;
            let (rgb_row, _) = output_rgb[start..start + row_bytes].as_chunks_mut::<3>();
            let (rgba_row, _) = rgba_row.as_chunks::<4>();
            for (&[r, g, b, _], rgb) in rgba_row.iter().zip(rgb_row) {
                *rgb = [r, g, b];
            }
        }
    }
}

/// An RGB24 frame whose rows start every `stride` bytes.
#[derive(Clone, Copy)]
struct Strided<'a> {
    data: &'a [u8],
    stride: usize,
}

impl<'a> Strided<'a> {
    fn new(data: &'a [u8], stride: usize) -> Self {
        Self { data, stride }
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

    #[test]
    fn padded_rows_match_packed_frames_in_every_chain() {
        let (w, h) = (30, 18);
        let row = w * 3;
        let (in_stride, out_stride) = (row + 6, row + 2);
        let packed = test_frame(w, h);
        let mut padded = vec![0xAB; in_stride * h];
        for y in 0..h {
            padded[y * in_stride..y * in_stride + row].copy_from_slice(&packed[y * row..][..row]);
        }
        let chains = [
            (true, BendOrder::Before, EffectSettings::default()),
            (false, BendOrder::Before, inverted()),
            (true, BendOrder::Before, inverted()),
            (true, BendOrder::After, inverted()),
            (false, BendOrder::Before, EffectSettings::default()),
        ];
        for (ntsc, bend_order, effect) in chains {
            let chain = ChainLayout { ntsc, bend_order };
            let expected = run(chain, &effect, &packed, w, h);
            let mut fp = FrameProcessor::new(w, h, chain);
            let mut out = vec![0x5C; out_stride * h];
            fp.process_strided(&effect, 0, &padded, in_stride, &mut out, out_stride);
            for y in 0..h {
                let line = &out[y * out_stride..(y + 1) * out_stride];
                assert_eq!(
                    &line[..row],
                    &expected[y * row..][..row],
                    "{chain:?} row {y}"
                );
                assert!(
                    line[row..].iter().all(|&b| b == 0x5C),
                    "{chain:?} padding written"
                );
            }
        }
    }

    #[test]
    fn bgra_processing_matches_rgb_and_keeps_alpha() {
        let (w, h) = (64, 48);
        let rgb = test_frame(w, h);
        let mut bgra = vec![0u8; w * h * 4];
        for (i, (&[r, g, b], px)) in rgb
            .as_chunks::<3>()
            .0
            .iter()
            .zip(bgra.as_chunks_mut::<4>().0)
            .enumerate()
        {
            *px = [b, g, r, (i % w) as u8];
        }
        let effect = inverted();
        let expected = {
            let mut fp = FrameProcessor::new(w, h, ChainLayout::default());
            let mut out = vec![0u8; fp.frame_bytes()];
            fp.process(&effect, 5, &rgb, &mut out);
            out
        };
        let mut fp = FrameProcessor::new(w, h, ChainLayout::default());
        let mut out = vec![0u8; w * h * 4];
        fp.process_bgra(&effect, 5, &bgra, w * 4, &mut out, w * 4);
        let (expected, _) = expected.as_chunks::<3>();
        let (got, _) = out.as_chunks::<4>();
        for (i, (&[r, g, b], &[ob, og, or, oa])) in expected.iter().zip(got).enumerate() {
            assert_eq!([ob, og, or], [b, g, r], "pixel {i}");
            assert_eq!(oa, (i % w) as u8, "alpha {i}");
        }
    }
}
