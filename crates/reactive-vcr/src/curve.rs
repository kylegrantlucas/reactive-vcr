use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve {
    Linear,
    /// x^k, k > 0
    Pow(f32),
    /// (e^(4x) - 1) / (e^4 - 1): slow start, fast finish
    Exp,
    /// ln(1 + 9x) / ln(10): fast start, slow finish
    Log,
}

impl Curve {
    pub fn parse(s: &str) -> Result<Curve> {
        let s = s.trim();
        if let Some(k) = s.strip_prefix("pow:") {
            let k: f32 = k
                .parse()
                .map_err(|_| anyhow::anyhow!("curve pow:<k> needs a number, got {k:?}"))?;
            if !k.is_finite() || k <= 0.0 {
                bail!("curve pow exponent must be finite and > 0, got {k}");
            }
            return Ok(Curve::Pow(k));
        }
        match s {
            "linear" => Ok(Curve::Linear),
            "exp" => Ok(Curve::Exp),
            "log" => Ok(Curve::Log),
            other => bail!("unknown curve {other:?}; expected linear | pow:<k> | exp | log"),
        }
    }

    pub fn apply(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        let y = match self {
            Curve::Linear => x,
            Curve::Pow(k) => x.powf(*k),
            Curve::Exp => ((4.0 * x).exp() - 1.0) / (4.0f32.exp() - 1.0),
            Curve::Log => (1.0 + 9.0 * x).ln() / 10.0f32.ln(),
        };
        y.clamp(0.0, 1.0)
    }
}

pub fn lerp(range: (f32, f32), x: f32) -> f32 {
    range.0 + (range.1 - range.0) * x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_variants() {
        assert!(matches!(Curve::parse("linear").unwrap(), Curve::Linear));
        assert!(matches!(Curve::parse("pow:2").unwrap(), Curve::Pow(k) if (k - 2.0).abs() < 1e-6));
        assert!(matches!(Curve::parse("exp").unwrap(), Curve::Exp));
        assert!(matches!(Curve::parse("log").unwrap(), Curve::Log));
        assert!(Curve::parse("wat").is_err());
        assert!(Curve::parse("pow:abc").is_err());
        assert!(Curve::parse("pow:0").is_err());
        assert!(Curve::parse("pow:NaN").is_err());
        assert!(Curve::parse("pow:inf").is_err());
    }

    #[test]
    fn endpoints_are_fixed_and_monotonic() {
        for c in [
            Curve::Linear,
            Curve::Pow(2.0),
            Curve::Pow(0.5),
            Curve::Exp,
            Curve::Log,
        ] {
            assert!((c.apply(0.0)).abs() < 1e-5, "{c:?} at 0");
            assert!((c.apply(1.0) - 1.0).abs() < 1e-5, "{c:?} at 1");
            let mut prev = -1.0;
            for i in 0..=100 {
                let y = c.apply(i as f32 / 100.0);
                assert!(y >= prev - 1e-6, "{c:?} not monotonic at {i}");
                prev = y;
            }
        }
        assert!(Curve::Pow(2.0).apply(0.5) < 0.5);
        assert!(Curve::Log.apply(0.5) > 0.5);
        assert!(Curve::Exp.apply(0.5) < 0.5);
        assert_eq!(Curve::Linear.apply(1.7), 1.0);
    }

    #[test]
    fn lerp_maps_range() {
        assert_eq!(lerp((2.0, 30.0), 0.0), 2.0);
        assert_eq!(lerp((2.0, 30.0), 1.0), 30.0);
        assert_eq!(lerp((10.0, 0.0), 0.25), 7.5);
    }
}
