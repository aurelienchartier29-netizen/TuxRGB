//! Material 3 Expressive theming driven by the desktop accent colour.
//!
//! The KDE Plasma accent (via the freedesktop Settings portal, falling back
//! to `kdeglobals`) seeds a Material dynamic colour scheme (HCT, "Expressive"
//! variant by default). Theme changes cross-fade with an effects spring.

// Windows build: the same interface, fed by the registry / WinRT.
#[cfg_attr(windows, path = "windows.rs")]
pub mod kde;
pub mod motion;
pub mod style;
pub mod tokens;

use crate::config::SchemeVariant;
use iced::Color;
use material_colors::color::Rgb as McRgb;
use material_colors::dynamic_color::{DynamicScheme, Variant};
use material_colors::scheme::Scheme;
use tuxrgb_core::Rgb;

/// Default seed when the desktop exposes no accent colour (KDE Breeze blue).
pub const BREEZE_ACCENT: Rgb = Rgb::new(61, 174, 233);

/// Material colour roles used by TuxRGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Roles {
    pub dark: bool,
    pub primary: Color,
    pub on_primary: Color,
    pub primary_container: Color,
    pub on_primary_container: Color,
    pub secondary: Color,
    pub on_secondary: Color,
    pub secondary_container: Color,
    pub on_secondary_container: Color,
    pub tertiary: Color,
    pub on_tertiary: Color,
    pub tertiary_container: Color,
    pub on_tertiary_container: Color,
    pub error: Color,
    pub on_error: Color,
    pub error_container: Color,
    pub on_error_container: Color,
    pub surface: Color,
    pub surface_dim: Color,
    pub surface_bright: Color,
    pub surface_container_lowest: Color,
    pub surface_container_low: Color,
    pub surface_container: Color,
    pub surface_container_high: Color,
    pub surface_container_highest: Color,
    pub on_surface: Color,
    pub on_surface_variant: Color,
    pub outline: Color,
    pub outline_variant: Color,
    pub inverse_surface: Color,
    pub inverse_on_surface: Color,
    pub inverse_primary: Color,
    pub scrim: Color,
    pub shadow: Color,
    pub success: Color,
    pub warning: Color,
}

fn c(rgb: McRgb) -> Color {
    Color::from_rgb8(rgb.red, rgb.green, rgb.blue)
}

impl Roles {
    pub fn generate(seed: Rgb, dark: bool, variant: SchemeVariant) -> Self {
        let variant = match variant {
            SchemeVariant::Expressive => Variant::Expressive,
            SchemeVariant::TonalSpot => Variant::TonalSpot,
            SchemeVariant::Vibrant => Variant::Vibrant,
            SchemeVariant::Fidelity => Variant::Fidelity,
            SchemeVariant::Neutral => Variant::Neutral,
        };
        let dynamic = DynamicScheme::by_variant(
            McRgb::new(seed.r, seed.g, seed.b),
            &variant,
            dark,
            Some(0.0),
        );
        let s: Scheme = dynamic.into();
        let (success, warning) = if dark {
            (
                Color::from_rgb8(0x8b, 0xd8, 0x8f),
                Color::from_rgb8(0xf5, 0xc5, 0x6b),
            )
        } else {
            (
                Color::from_rgb8(0x1b, 0x6c, 0x2f),
                Color::from_rgb8(0x7a, 0x59, 0x00),
            )
        };
        Self {
            dark,
            primary: c(s.primary),
            on_primary: c(s.on_primary),
            primary_container: c(s.primary_container),
            on_primary_container: c(s.on_primary_container),
            secondary: c(s.secondary),
            on_secondary: c(s.on_secondary),
            secondary_container: c(s.secondary_container),
            on_secondary_container: c(s.on_secondary_container),
            tertiary: c(s.tertiary),
            on_tertiary: c(s.on_tertiary),
            tertiary_container: c(s.tertiary_container),
            on_tertiary_container: c(s.on_tertiary_container),
            error: c(s.error),
            on_error: c(s.on_error),
            error_container: c(s.error_container),
            on_error_container: c(s.on_error_container),
            surface: c(s.surface),
            surface_dim: c(s.surface_dim),
            surface_bright: c(s.surface_bright),
            surface_container_lowest: c(s.surface_container_lowest),
            surface_container_low: c(s.surface_container_low),
            surface_container: c(s.surface_container),
            surface_container_high: c(s.surface_container_high),
            surface_container_highest: c(s.surface_container_highest),
            on_surface: c(s.on_surface),
            on_surface_variant: c(s.on_surface_variant),
            outline: c(s.outline),
            outline_variant: c(s.outline_variant),
            inverse_surface: c(s.inverse_surface),
            inverse_on_surface: c(s.inverse_on_surface),
            inverse_primary: c(s.inverse_primary),
            scrim: c(s.scrim),
            shadow: c(s.shadow),
            success,
            warning,
        }
    }

    /// Per-role linear interpolation, for theme cross-fades.
    /// Every colour with its alpha multiplied by `alpha`: whole components
    /// (dialogs) fade in and out for real, text and icons included.
    pub fn faded(&self, alpha: f32) -> Roles {
        let a = alpha.clamp(0.0, 1.0);
        let f = |c: Color| Color { a: c.a * a, ..c };
        Roles {
            dark: self.dark,
            primary: f(self.primary),
            on_primary: f(self.on_primary),
            primary_container: f(self.primary_container),
            on_primary_container: f(self.on_primary_container),
            secondary: f(self.secondary),
            on_secondary: f(self.on_secondary),
            secondary_container: f(self.secondary_container),
            on_secondary_container: f(self.on_secondary_container),
            tertiary: f(self.tertiary),
            on_tertiary: f(self.on_tertiary),
            tertiary_container: f(self.tertiary_container),
            on_tertiary_container: f(self.on_tertiary_container),
            error: f(self.error),
            on_error: f(self.on_error),
            error_container: f(self.error_container),
            on_error_container: f(self.on_error_container),
            surface: f(self.surface),
            surface_dim: f(self.surface_dim),
            surface_bright: f(self.surface_bright),
            surface_container_lowest: f(self.surface_container_lowest),
            surface_container_low: f(self.surface_container_low),
            surface_container: f(self.surface_container),
            surface_container_high: f(self.surface_container_high),
            surface_container_highest: f(self.surface_container_highest),
            on_surface: f(self.on_surface),
            on_surface_variant: f(self.on_surface_variant),
            outline: f(self.outline),
            outline_variant: f(self.outline_variant),
            inverse_surface: f(self.inverse_surface),
            inverse_on_surface: f(self.inverse_on_surface),
            inverse_primary: f(self.inverse_primary),
            scrim: f(self.scrim),
            shadow: f(self.shadow),
            success: f(self.success),
            warning: f(self.warning),
        }
    }

    pub fn lerp(a: &Roles, b: &Roles, t: f32) -> Roles {
        let t = t.clamp(0.0, 1.0);
        let l = |x: Color, y: Color| mix(x, y, t);
        Roles {
            dark: if t < 0.5 { a.dark } else { b.dark },
            primary: l(a.primary, b.primary),
            on_primary: l(a.on_primary, b.on_primary),
            primary_container: l(a.primary_container, b.primary_container),
            on_primary_container: l(a.on_primary_container, b.on_primary_container),
            secondary: l(a.secondary, b.secondary),
            on_secondary: l(a.on_secondary, b.on_secondary),
            secondary_container: l(a.secondary_container, b.secondary_container),
            on_secondary_container: l(a.on_secondary_container, b.on_secondary_container),
            tertiary: l(a.tertiary, b.tertiary),
            on_tertiary: l(a.on_tertiary, b.on_tertiary),
            tertiary_container: l(a.tertiary_container, b.tertiary_container),
            on_tertiary_container: l(a.on_tertiary_container, b.on_tertiary_container),
            error: l(a.error, b.error),
            on_error: l(a.on_error, b.on_error),
            error_container: l(a.error_container, b.error_container),
            on_error_container: l(a.on_error_container, b.on_error_container),
            surface: l(a.surface, b.surface),
            surface_dim: l(a.surface_dim, b.surface_dim),
            surface_bright: l(a.surface_bright, b.surface_bright),
            surface_container_lowest: l(a.surface_container_lowest, b.surface_container_lowest),
            surface_container_low: l(a.surface_container_low, b.surface_container_low),
            surface_container: l(a.surface_container, b.surface_container),
            surface_container_high: l(a.surface_container_high, b.surface_container_high),
            surface_container_highest: l(a.surface_container_highest, b.surface_container_highest),
            on_surface: l(a.on_surface, b.on_surface),
            on_surface_variant: l(a.on_surface_variant, b.on_surface_variant),
            outline: l(a.outline, b.outline),
            outline_variant: l(a.outline_variant, b.outline_variant),
            inverse_surface: l(a.inverse_surface, b.inverse_surface),
            inverse_on_surface: l(a.inverse_on_surface, b.inverse_on_surface),
            inverse_primary: l(a.inverse_primary, b.inverse_primary),
            scrim: l(a.scrim, b.scrim),
            shadow: l(a.shadow, b.shadow),
            success: l(a.success, b.success),
            warning: l(a.warning, b.warning),
        }
    }

    /// iced theme carrying the palette for widgets without a custom style.
    pub fn iced_theme(&self) -> iced::Theme {
        iced::Theme::custom(
            if self.dark {
                "TuxRGB sombre"
            } else {
                "TuxRGB clair"
            },
            iced::theme::Palette {
                background: self.surface,
                text: self.on_surface,
                primary: self.primary,
                success: self.success,
                warning: self.warning,
                danger: self.error,
            },
        )
    }
}

/// Mixes two colours (`t` = 0 → `a`).
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// `c` at opacity `a` (multiplied: a colour that is already fading keeps
/// fading, see [`Roles::faded`]).
pub fn with_alpha(c: Color, a: f32) -> Color {
    Color { a: c.a * a, ..c }
}

pub fn to_iced(c: Rgb) -> Color {
    Color::from_rgb8(c.r, c.g, c.b)
}

#[cfg(test)]
pub fn from_iced(c: Color) -> Rgb {
    Rgb::from_f32(c.r, c.g, c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Color, b: Color) -> f32 {
        let la = from_iced(a).luminance() + 0.05;
        let lb = from_iced(b).luminance() + 0.05;
        la.max(lb) / la.min(lb)
    }

    #[test]
    fn generated_roles_are_readable() {
        for dark in [false, true] {
            for seed in [BREEZE_ACCENT, Rgb::new(233, 30, 99), Rgb::new(255, 235, 59)] {
                let r = Roles::generate(seed, dark, SchemeVariant::Expressive);
                assert!(contrast(r.on_surface, r.surface) >= 4.5);
                assert!(contrast(r.on_primary, r.primary) >= 3.0);
                assert!(contrast(r.on_primary_container, r.primary_container) >= 4.5);
                assert_eq!(r.dark, dark);
            }
        }
    }
}
