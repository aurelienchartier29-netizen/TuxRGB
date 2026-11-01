//! Spring physics for M3 Expressive motion.
//!
//! Springs keep their velocity when the target changes mid-flight, which is
//! what makes expressive motion feel physical (Bézier tweens restart).
//! The GUI only subscribes to frame events while a spring is moving, so an
//! idle window costs zero CPU.

use super::tokens::SpringSpec;
use std::sync::atomic::{AtomicU32, Ordering};

/// Speed of every interface animation (Réglages → Vitesse des animations):
/// animation clocks run this many times faster than real time.
static TIME_SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0

/// Range of the setting (25 % to 300 %).
pub const SPEED_MIN: f32 = 0.25;
pub const SPEED_MAX: f32 = 3.0;

pub fn set_time_scale(speed: f32) {
    let s = if speed.is_finite() { speed } else { 1.0 };
    TIME_SCALE.store(s.clamp(SPEED_MIN, SPEED_MAX).to_bits(), Ordering::Relaxed);
}

pub fn time_scale() -> f32 {
    f32::from_bits(TIME_SCALE.load(Ordering::Relaxed))
}

/// Frame time of an animation at the chosen animation speed.
#[inline]
pub fn scaled(dt: f32) -> f32 {
    dt * time_scale()
}

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    value: f32,
    velocity: f32,
    target: f32,
    spec: SpringSpec,
    moving: bool,
}

impl Spring {
    pub fn new(value: f32, spec: SpringSpec) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
            spec,
            moving: false,
        }
    }

    #[inline]
    pub fn value(&self) -> f32 {
        self.value
    }

    /// The value it is heading to.
    #[inline]
    pub fn target(&self) -> f32 {
        self.target
    }

    #[inline]
    pub fn is_moving(&self) -> bool {
        self.moving
    }

    pub fn set_spec(&mut self, spec: SpringSpec) {
        self.spec = spec;
    }

    /// Animates towards `target` (keeps the current velocity).
    pub fn set_target(&mut self, target: f32) {
        if (target - self.target).abs() > f32::EPSILON {
            self.target = target;
            self.moving = true;
        }
    }

    /// Jumps to `value` without animation.
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
        self.moving = false;
    }

    /// Restarts from `from` towards `to`.
    pub fn restart(&mut self, from: f32, to: f32) {
        self.value = from;
        self.velocity = 0.0;
        self.target = to;
        self.moving = (from - to).abs() > f32::EPSILON;
    }

    /// Advances the simulation; returns `true` while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if !self.moving {
            return false;
        }
        let k = self.spec.stiffness;
        let c = 2.0 * self.spec.damping * k.sqrt();
        // Fixed sub-steps keep stiff springs stable at any frame rate.
        let mut remaining = dt.clamp(0.0, 0.3);
        const H: f32 = 1.0 / 480.0;
        while remaining > 0.0 {
            let h = remaining.min(H);
            let a = -k * (self.value - self.target) - c * self.velocity;
            self.velocity += a * h;
            self.value += self.velocity * h;
            remaining -= h;
        }
        let range = 1.0f32.max(self.target.abs());
        if (self.value - self.target).abs() < 0.0005 * range && self.velocity.abs() < 0.005 * range
        {
            self.value = self.target;
            self.velocity = 0.0;
            self.moving = false;
        }
        self.moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::tokens::motion;

    fn settle(mut s: Spring) -> (f32, f32) {
        let mut t = 0.0;
        let mut peak = s.value();
        while s.step(1.0 / 60.0) {
            t += 1.0 / 60.0;
            peak = peak.max(s.value());
            assert!(t < 5.0, "spring never settled");
        }
        (t, peak)
    }

    #[test]
    fn expressive_spatial_overshoots_then_settles() {
        let mut s = Spring::new(0.0, motion::EXPRESSIVE_FAST_SPATIAL);
        s.set_target(1.0);
        let (t, peak) = settle(s);
        assert!(peak > 1.02, "expected overshoot, peak={peak}");
        assert!(t < 1.0, "settle time {t}");
    }

    #[test]
    fn animation_speed_scales_time_and_is_bounded() {
        set_time_scale(10.0);
        assert_eq!(time_scale(), SPEED_MAX);
        set_time_scale(0.0);
        assert_eq!(time_scale(), SPEED_MIN);
        set_time_scale(f32::NAN);
        assert_eq!(time_scale(), 1.0);
        // At 200 % a spring driven by scaled frames settles twice as fast.
        let settle_with = |speed: f32| {
            set_time_scale(speed);
            let mut s = Spring::new(0.0, motion::DEFAULT_EFFECTS);
            s.set_target(1.0);
            let mut frames = 0;
            while s.step(scaled(1.0 / 60.0)) {
                frames += 1;
            }
            frames
        };
        let (normal, fast) = (settle_with(1.0), settle_with(2.0));
        set_time_scale(1.0);
        assert!(
            (fast as f32 - normal as f32 / 2.0).abs() <= 1.5,
            "{normal} / {fast}"
        );
    }

    #[test]
    fn effects_spring_never_overshoots() {
        let mut s = Spring::new(0.0, motion::FAST_EFFECTS);
        s.set_target(1.0);
        let (t, peak) = settle(s);
        assert!(peak <= 1.0001, "peak={peak}");
        assert!(t < 0.4, "settle time {t}");
    }
}
