//! # territorial_render
//!
//! Map rasterizer, camera, themes, terminal capability detection and ASCII art
//! for Territorial CLI.
//!
//! * [`caps`]: [`Capabilities::detect`] and the user's [`RendererChoice`].
//! * [`theme`]: [`Theme`] (dark, high contrast, colorblind safe), the player palette
//!   and color quantization to 256 and 16 colors.
//! * [`camera`]: [`Camera`] with three [`ZoomLevel`]s and horizontal wrap-around.
//! * [`map_render`]: [`MapRenderer`], the half-block and ASCII map renderers.
//! * [`art`]: splash frames, banners, countdown digits and event-log flourishes.
//!
//! Rendering is not part of the deterministic simulation, so this crate uses
//! floating point freely.

pub mod art;
pub mod camera;
pub mod caps;
pub mod map_render;
pub mod theme;

pub use camera::{Camera, ZoomLevel};
pub use caps::{Capabilities, ColorMode, GlyphMode, RendererChoice};
pub use map_render::{MapRenderCtx, MapRenderer};
pub use theme::{quantize, region_color, to_ansi16, to_ansi256, Rgb, Theme, ThemeKind};

/// Formats a troop count compactly: `950`, `1.2k`, `45k`, `3.4M`, `120M`.
pub fn compact_number(n: i64) -> String {
    let neg = n < 0;
    let v = n.unsigned_abs();
    let body = if v < 1_000 {
        v.to_string()
    } else if v < 10_000 {
        format!("{}.{}k", v / 1_000, (v % 1_000) / 100)
    } else if v < 1_000_000 {
        format!("{}k", v / 1_000)
    } else if v < 10_000_000 {
        format!("{}.{}M", v / 1_000_000, (v % 1_000_000) / 100_000)
    } else {
        format!("{}M", v / 1_000_000)
    };
    if neg {
        format!("-{}", body)
    } else {
        body
    }
}

/// Formats a number with thousands separators: `12480` becomes `12,480`.
pub fn grouped_number(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    let len = digits.len();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
