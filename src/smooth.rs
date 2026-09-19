/// One-pole attack/release smoother running at frame rate.
#[derive(Debug, Clone)]
pub struct AttackRelease {
    attack: f32,
    release: f32,
    y: f32,
}

fn coef(ms: f32, fps: f64) -> f32 {
    if ms <= 0.0 {
        return 0.0;
    }
    let frames = (ms as f64 / 1000.0) * fps;
    (-1.0 / frames).exp() as f32
}

impl AttackRelease {
    pub fn new(attack_ms: f32, release_ms: f32, fps: f64) -> Self {
        Self {
            attack: coef(attack_ms, fps),
            release: coef(release_ms, fps),
            y: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.y = 0.0;
    }

    pub fn process(&mut self, x: f32) -> f32 {
        let c = if x > self.y {
            self.attack
        } else {
            self.release
        };
        self.y = x + c * (self.y - x);
        self.y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_times_are_instant() {
        let mut s = AttackRelease::new(0.0, 0.0, 30.0);
        assert_eq!(s.process(1.0), 1.0);
        assert_eq!(s.process(0.0), 0.0);
    }

    #[test]
    fn release_decays_toward_input() {
        let mut s = AttackRelease::new(0.0, 200.0, 30.0);
        assert_eq!(s.process(1.0), 1.0);
        let a = s.process(0.0);
        let b = s.process(0.0);
        assert!(a > b && b > 0.0 && a < 1.0);
        for _ in 0..200 {
            s.process(0.0);
        }
        assert!(s.process(0.0) < 0.01);
    }

    #[test]
    fn attack_rises_gradually() {
        let mut s = AttackRelease::new(100.0, 0.0, 30.0);
        let a = s.process(1.0);
        let b = s.process(1.0);
        assert!(a > 0.0 && a < 1.0 && b > a);
    }
}
