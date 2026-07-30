//! Basic-color-space conversion (ISO 32000-1:2008 8.6.3 "DeviceGray Color
//! Space", 8.6.4.2 "DeviceRGB Color Space", 8.6.5 "DeviceCMYK Color
//! Space") into a `tiny-skia` [`Color`].
//!
//! # Scope and honest limitations
//!
//! This phase only implements the three "Device" color spaces set via the
//! `g`/`G`, `rg`/`RG`, `k`/`K` operators. It does **not** implement
//! CalGray/CalRGB/Lab (8.6.5.2-ish calibrated spaces), ICCBased,
//! Indexed, Separation/DeviceN, or Pattern color spaces (`cs`/`CS`/`sc`/
//! `SC`/`scn`/`SCN`) -- those are a later phase (see the module docs of
//! [`crate::render::native`]) and are recorded as
//! [`RenderWarning::UnsupportedOperator`](super::error::RenderWarning::UnsupportedOperator)
//! rather than silently guessed at.
//!
//! The DeviceCMYK -> RGB conversion below is the naive, non-color-managed
//! formula ISO 32000-1 8.6.5.3 itself gives as the default conversion in
//! the absence of a more specific color space substitute:
//! `red = 1.0 - min(1.0, cyan + black)` (and similarly for green/blue). It
//! is **not** ICC-profile-based color management -- there is no mature
//! pure-Rust ICC color management engine this crate could adopt, so
//! accurate/perceptual CMYK->RGB conversion (matching e.g. a specific
//! press profile) is an explicitly out-of-scope gap, not something this
//! module claims to solve.

use tiny_skia::Color;

/// Clamps a PDF color component to the valid `[0.0, 1.0]` range,
/// substituting `0.0` for non-finite input (a malformed/adversarial
/// content stream must not be able to smuggle a `NaN`/`Infinity` into the
/// rasterizer).
fn clamp_component(v: f64) -> f32 {
    if !v.is_finite() {
        0.0
    } else {
        v.clamp(0.0, 1.0) as f32
    }
}

/// Converts a DeviceGray component (ISO 32000-1 8.6.3, `0.0` = black,
/// `1.0` = white) plus a constant alpha to an opaque-channel RGBA
/// [`Color`].
pub(super) fn device_gray(gray: f64, alpha: f32) -> Color {
    let v = clamp_component(gray);
    Color::from_rgba(v, v, v, alpha.clamp(0.0, 1.0)).unwrap_or(Color::BLACK)
}

/// Converts DeviceRGB components (ISO 32000-1 8.6.4.2) plus a constant
/// alpha to a [`Color`].
pub(super) fn device_rgb(r: f64, g: f64, b: f64, alpha: f32) -> Color {
    Color::from_rgba(
        clamp_component(r),
        clamp_component(g),
        clamp_component(b),
        alpha.clamp(0.0, 1.0),
    )
    .unwrap_or(Color::BLACK)
}

/// Converts DeviceCMYK components (ISO 32000-1 8.6.5) plus a constant
/// alpha to a [`Color`], using the spec's own naive (non-ICC) conversion
/// formula. See the module docs for why this is an intentional
/// simplification rather than true color management.
pub(super) fn device_cmyk(c: f64, m: f64, y: f64, k: f64, alpha: f32) -> Color {
    let c = clamp_component(c) as f64;
    let m = clamp_component(m) as f64;
    let y = clamp_component(y) as f64;
    let k = clamp_component(k) as f64;
    let r = 1.0 - (c + k).min(1.0);
    let g = 1.0 - (m + k).min(1.0);
    let b = 1.0 - (y + k).min(1.0);
    Color::from_rgba(r as f32, g as f32, b as f32, alpha.clamp(0.0, 1.0)).unwrap_or(Color::BLACK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_black_and_white() {
        let black = device_gray(0.0, 1.0);
        assert_eq!((black.red(), black.green(), black.blue()), (0.0, 0.0, 0.0));
        let white = device_gray(1.0, 1.0);
        assert_eq!((white.red(), white.green(), white.blue()), (1.0, 1.0, 1.0));
    }

    #[test]
    fn gray_clamps_out_of_range() {
        let over = device_gray(2.0, 1.0);
        assert_eq!(over.red(), 1.0);
        let under = device_gray(-5.0, 1.0);
        assert_eq!(under.red(), 0.0);
    }

    #[test]
    fn gray_rejects_non_finite() {
        let c = device_gray(f64::NAN, 1.0);
        assert_eq!((c.red(), c.green(), c.blue()), (0.0, 0.0, 0.0));
    }

    #[test]
    fn rgb_pure_red() {
        let red = device_rgb(1.0, 0.0, 0.0, 1.0);
        assert_eq!((red.red(), red.green(), red.blue()), (1.0, 0.0, 0.0));
    }

    #[test]
    fn cmyk_pure_cyan_is_no_red() {
        // C=1 M=0 Y=0 K=0 -> red = 1 - min(1, 1+0) = 0, green = blue = 1.
        let cyan = device_cmyk(1.0, 0.0, 0.0, 0.0, 1.0);
        assert_eq!((cyan.red(), cyan.green(), cyan.blue()), (0.0, 1.0, 1.0));
    }

    #[test]
    fn cmyk_all_zero_is_white() {
        let white = device_cmyk(0.0, 0.0, 0.0, 0.0, 1.0);
        assert_eq!((white.red(), white.green(), white.blue()), (1.0, 1.0, 1.0));
    }

    #[test]
    fn cmyk_full_black_key_is_black() {
        let black = device_cmyk(0.0, 0.0, 0.0, 1.0, 1.0);
        assert_eq!((black.red(), black.green(), black.blue()), (0.0, 0.0, 0.0));
    }

    #[test]
    fn alpha_is_applied() {
        let c = device_rgb(1.0, 1.0, 1.0, 0.5);
        assert_eq!(c.alpha(), 0.5);
    }
}
