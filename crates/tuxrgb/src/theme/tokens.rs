#![allow(dead_code)] // complete M3 token table, not every token is used yet
//! Material 3 Expressive design tokens.
//!
//! Values follow the M3 Expressive update (May 2025): extended shape scale,
//! spring-based motion schemes and the emphasized type styles.

/// Corner radii in logical pixels (dp).
pub mod shape {
    pub const EXTRA_SMALL: f32 = 4.0;
    pub const SMALL: f32 = 8.0;
    pub const MEDIUM: f32 = 12.0;
    pub const LARGE: f32 = 16.0;
    pub const LARGE_INCREASED: f32 = 20.0;
    pub const EXTRA_LARGE: f32 = 28.0;
    pub const EXTRA_LARGE_INCREASED: f32 = 32.0;
    pub const EXTRA_EXTRA_LARGE: f32 = 48.0;
    /// Fully rounded (pill); iced clamps the radius to half the height.
    pub const FULL: f32 = 1000.0;
}

/// Spacing scale.
pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
    pub const XXXL: f32 = 48.0;
}

/// Type scale (size, line height) in sp. Emphasized variants use a heavier
/// weight, see [`crate::ui::widgets`].
pub mod typescale {
    pub const DISPLAY_LARGE: f32 = 57.0;
    pub const DISPLAY_MEDIUM: f32 = 45.0;
    pub const DISPLAY_SMALL: f32 = 36.0;
    pub const HEADLINE_LARGE: f32 = 32.0;
    pub const HEADLINE_MEDIUM: f32 = 28.0;
    pub const HEADLINE_SMALL: f32 = 24.0;
    pub const TITLE_LARGE: f32 = 22.0;
    pub const TITLE_MEDIUM: f32 = 16.0;
    pub const TITLE_SMALL: f32 = 14.0;
    pub const BODY_LARGE: f32 = 16.0;
    pub const BODY_MEDIUM: f32 = 14.0;
    pub const BODY_SMALL: f32 = 12.0;
    pub const LABEL_LARGE: f32 = 14.0;
    pub const LABEL_MEDIUM: f32 = 12.0;
    pub const LABEL_SMALL: f32 = 11.0;
}

/// Spring parameters (damping ratio, stiffness) of the M3 motion schemes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringSpec {
    pub damping: f32,
    pub stiffness: f32,
}

pub mod motion {
    use super::SpringSpec;

    // Expressive scheme – spatial springs may overshoot.
    pub const EXPRESSIVE_FAST_SPATIAL: SpringSpec = SpringSpec {
        damping: 0.6,
        stiffness: 800.0,
    };
    pub const EXPRESSIVE_DEFAULT_SPATIAL: SpringSpec = SpringSpec {
        damping: 0.8,
        stiffness: 380.0,
    };
    pub const EXPRESSIVE_SLOW_SPATIAL: SpringSpec = SpringSpec {
        damping: 0.8,
        stiffness: 200.0,
    };
    // Standard scheme – calmer, used when reduced motion is requested.
    pub const STANDARD_FAST_SPATIAL: SpringSpec = SpringSpec {
        damping: 0.9,
        stiffness: 1400.0,
    };
    pub const STANDARD_DEFAULT_SPATIAL: SpringSpec = SpringSpec {
        damping: 0.9,
        stiffness: 700.0,
    };
    pub const STANDARD_SLOW_SPATIAL: SpringSpec = SpringSpec {
        damping: 0.9,
        stiffness: 300.0,
    };
    // Effects (colour, opacity) never overshoot – shared by both schemes.
    pub const FAST_EFFECTS: SpringSpec = SpringSpec {
        damping: 1.0,
        stiffness: 3800.0,
    };
    pub const DEFAULT_EFFECTS: SpringSpec = SpringSpec {
        damping: 1.0,
        stiffness: 1600.0,
    };
    pub const SLOW_EFFECTS: SpringSpec = SpringSpec {
        damping: 1.0,
        stiffness: 800.0,
    };
}

/// Minimum touch / click target.
pub const MIN_TARGET: f32 = 48.0;
/// Navigation rail width (M3 Expressive collapsed rail).
pub const RAIL_WIDTH: f32 = 96.0;
