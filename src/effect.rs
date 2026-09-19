use ntsc_rs::NtscEffect;
use ntsc_rs::settings::UseField;
use ntsc_rs::yiq_fielding::{BlitInfo, DeinterlaceMode, Rgb, YiqView, pixel_bytes_for};

/// Applies an `NtscEffect` to tightly packed RGB24 frames, reusing one YIQ scratch buffer.
pub struct FrameProcessor {
    width: usize,
    height: usize,
    scratch: Vec<f32>,
}

impl FrameProcessor {
    pub fn new(width: usize, height: usize) -> Self {
        let len = YiqView::max_buf_length_for((width, height), UseField::Both);
        Self {
            width,
            height,
            scratch: vec![0.0; len],
        }
    }

    pub fn frame_bytes(&self) -> usize {
        self.width * self.height * pixel_bytes_for::<Rgb, u8>()
    }

    pub fn process(
        &mut self,
        effect: &NtscEffect,
        frame_num: usize,
        input: &[u8],
        output: &mut [u8],
    ) {
        assert_eq!(input.len(), self.frame_bytes(), "input frame size");
        assert_eq!(output.len(), self.frame_bytes(), "output frame size");

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{ParamCatalog, set_param};
    use ntsc_rs::NtscEffect;

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

    #[test]
    fn effect_changes_pixels_and_is_deterministic() {
        let (w, h) = (128, 72);
        let input = test_frame(w, h);
        let mut fp = FrameProcessor::new(w, h);
        assert_eq!(fp.frame_bytes(), w * h * 3);
        let mut a = vec![0u8; w * h * 3];
        let mut b = vec![0u8; w * h * 3];
        let e = NtscEffect::default();
        fp.process(&e, 0, &input, &mut a);
        fp.process(&e, 0, &input, &mut b);
        assert_eq!(a, b, "same effect + frame_num must be deterministic");
        assert_ne!(a, input, "effect should alter the image");
    }

    #[test]
    fn parameter_change_alters_output() {
        let (w, h) = (128, 72);
        let input = test_frame(w, h);
        let mut fp = FrameProcessor::new(w, h);
        let c = ParamCatalog::new();
        let mut loud = NtscEffect::default();
        set_param(&mut loud, c.get("snow_intensity").unwrap(), 50.0).unwrap();
        let mut a = vec![0u8; w * h * 3];
        let mut b = vec![0u8; w * h * 3];
        fp.process(&NtscEffect::default(), 3, &input, &mut a);
        fp.process(&loud, 3, &input, &mut b);
        assert_ne!(a, b);
    }

    #[test]
    fn every_field_mode_works_when_switching_on_reused_processor() {
        let (w, h) = (32, 18);
        let input = test_frame(w, h);
        let mut fp = FrameProcessor::new(w, h);
        let catalog = ParamCatalog::new();
        let field = catalog.get("use_field").unwrap();
        for mode in 0..6 {
            let mut effect = NtscEffect::default();
            set_param(&mut effect, field, mode as f64).unwrap();
            let mut output = vec![0u8; fp.frame_bytes()];
            fp.process(&effect, mode, &input, &mut output);
            assert_eq!(output.len(), w * h * 3);
        }
    }
}
