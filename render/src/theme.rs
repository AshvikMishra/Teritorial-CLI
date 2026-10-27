//! Color themes, the player palette and color quantization.
//!
//! Every color is defined as 24-bit [`Rgb`] and converted to a ratatui
//! [`Color`] for the active [`ColorMode`] with [`Theme::color`] (or [`quantize`]):
//! truecolor passes it through, 256-color maps it to the xterm 6x6x6 cube or the
//! grayscale ramp, and 16-color maps it to the nearest basic ANSI color.

use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use territorial_core::{Game, PlayerId, RegionId, MAX_BOTS};

use crate::caps::ColorMode;

/// The available themes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeKind {
    /// Dark background, balanced saturated player colors (the default).
    Dark,
    /// Black ocean, fully saturated colors and strong borders.
    #[serde(alias = "high-contrast", alias = "high_contrast")]
    HighContrast,
    /// Okabe-Ito based palette with lightness variants, safe for common color blindness.
    #[serde(alias = "colorblind", alias = "colorblind-safe", alias = "colorblind_safe")]
    ColorblindSafe,
}

impl Default for ThemeKind {
    fn default() -> ThemeKind {
        ThemeKind::Dark
    }
}

impl ThemeKind {
    /// Every theme, in menu order.
    pub const ALL: [ThemeKind; 3] = [
        ThemeKind::Dark,
        ThemeKind::HighContrast,
        ThemeKind::ColorblindSafe,
    ];

    /// The canonical lowercase name (`dark`, `high-contrast`, `colorblind`).
    pub fn name(self) -> &'static str {
        match self {
            ThemeKind::Dark => "dark",
            ThemeKind::HighContrast => "high-contrast",
            ThemeKind::ColorblindSafe => "colorblind",
        }
    }

    /// A display label (`Dark`, `High contrast`, `Colorblind safe`).
    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Dark => "Dark",
            ThemeKind::HighContrast => "High contrast",
            ThemeKind::ColorblindSafe => "Colorblind safe",
        }
    }

    /// Parses a theme name, case-insensitively. Accepts `dark`, `high-contrast`,
    /// `highcontrast`, `hc`, `contrast`, `colorblind`, `colorblind-safe`,
    /// `colorblindsafe`, `cb` and similar spellings.
    pub fn from_name(s: &str) -> Option<ThemeKind> {
        let lower = s.trim().to_ascii_lowercase();
        let squashed: String = lower
            .chars()
            .filter(|c| *c != '-' && *c != '_' && *c != ' ')
            .collect();
        match squashed.as_str() {
            "dark" | "default" => Some(ThemeKind::Dark),
            "highcontrast" | "hc" | "contrast" | "high" => Some(ThemeKind::HighContrast),
            "colorblind" | "colorblindsafe" | "cb" | "cbsafe" | "colourblind"
            | "colourblindsafe" | "okabeito" => Some(ThemeKind::ColorblindSafe),
            _ => None,
        }
    }

    /// The next theme in [`ThemeKind::ALL`], wrapping around.
    pub fn next(self) -> ThemeKind {
        match self {
            ThemeKind::Dark => ThemeKind::HighContrast,
            ThemeKind::HighContrast => ThemeKind::ColorblindSafe,
            ThemeKind::ColorblindSafe => ThemeKind::Dark,
        }
    }
}

/// A 24-bit color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Pure black.
    pub const BLACK: Rgb = Rgb(0, 0, 0);
    /// Pure white.
    pub const WHITE: Rgb = Rgb(255, 255, 255);

    /// Blends `pct` percent (0..=100) of `other` into `self`.
    pub fn mix(self, other: Rgb, pct: u8) -> Rgb {
        let p = u32::from(pct.min(100));
        let q = 100 - p;
        let ch = |a: u8, b: u8| -> u8 {
            let v = (u32::from(a) * q + u32::from(b) * p + 50) / 100;
            v.min(255) as u8
        };
        Rgb(ch(self.0, other.0), ch(self.1, other.1), ch(self.2, other.2))
    }

    /// Darkens toward black by `pct` percent.
    pub fn darken(self, pct: u8) -> Rgb {
        self.mix(Rgb::BLACK, pct)
    }

    /// Lightens toward white by `pct` percent.
    pub fn lighten(self, pct: u8) -> Rgb {
        self.mix(Rgb::WHITE, pct)
    }

    /// Perceived brightness, 0..=255 (Rec. 601 weights).
    pub fn luminance(self) -> u32 {
        (299 * u32::from(self.0) + 587 * u32::from(self.1) + 114 * u32::from(self.2)) / 1000
    }

    /// The 24-bit ratatui color, without quantization.
    pub fn to_color(self) -> Color {
        Color::Rgb(self.0, self.1, self.2)
    }

    /// Converts HSL to RGB. `h` is in degrees (any value, wrapped), `s` and `l` in `0.0..=1.0`.
    pub fn from_hsl(h: f32, s: f32, l: f32) -> Rgb {
        let h = h.rem_euclid(360.0) / 360.0;
        let s = s.clamp(0.0, 1.0);
        let l = l.clamp(0.0, 1.0);
        if s <= 0.0 {
            let v = to_u8(l);
            return Rgb(v, v, v);
        }
        let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
        let g = hue_to_rgb(p, q, h);
        let b = hue_to_rgb(p, q, h - 1.0 / 3.0);
        Rgb(to_u8(r), to_u8(g), to_u8(b))
    }

    /// Squared distance with rough perceptual weights (green counts most).
    fn dist2(self, other: Rgb) -> u32 {
        let dr = i32::from(self.0) - i32::from(other.0);
        let dg = i32::from(self.1) - i32::from(other.1);
        let db = i32::from(self.2) - i32::from(other.2);
        (2 * dr * dr + 4 * dg * dg + 3 * db * db) as u32
    }

    /// `max - min` over the channels: 0 for grays, high for vivid colors.
    fn chroma(self) -> u8 {
        let max = self.0.max(self.1).max(self.2);
        let min = self.0.min(self.1).min(self.2);
        max - min
    }
}

fn to_u8(v: f32) -> u8 {
    let x = (v * 255.0).round();
    if x <= 0.0 {
        0
    } else if x >= 255.0 {
        255
    } else {
        x as u8
    }
}

fn hue_to_rgb(p: f32, q: f32, t: f32) -> f32 {
    let mut t = t;
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

/// The six channel levels of the xterm 6x6x6 color cube.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn cube_index(v: u8) -> usize {
    if v < 48 {
        0
    } else if v < 115 {
        1
    } else {
        ((usize::from(v) - 35) / 40).min(5)
    }
}

/// Quantizes to the xterm 256-color palette: the nearest of the 6x6x6 cube
/// (indices 16..=231) and the 24-step grayscale ramp (232..=255).
pub fn to_ansi256(rgb: Rgb) -> u8 {
    let ri = cube_index(rgb.0);
    let gi = cube_index(rgb.1);
    let bi = cube_index(rgb.2);
    let cube = Rgb(CUBE_LEVELS[ri], CUBE_LEVELS[gi], CUBE_LEVELS[bi]);
    let cube_idx = 16 + 36 * ri + 6 * gi + bi;

    let avg = (u32::from(rgb.0) + u32::from(rgb.1) + u32::from(rgb.2)) / 3;
    let gray_i = if avg < 8 { 0 } else { ((avg - 8 + 5) / 10).min(23) };
    let gv = (8 + 10 * gray_i) as u8;
    let gray = Rgb(gv, gv, gv);
    let gray_idx = 232 + gray_i as usize;

    if rgb.dist2(gray) < rgb.dist2(cube) {
        gray_idx as u8
    } else {
        cube_idx as u8
    }
}

/// The 16 basic ANSI colors with their typical (xterm) RGB values.
const ANSI16: [(Color, Rgb); 16] = [
    (Color::Black, Rgb(0, 0, 0)),
    (Color::Red, Rgb(205, 0, 0)),
    (Color::Green, Rgb(0, 205, 0)),
    (Color::Yellow, Rgb(205, 205, 0)),
    (Color::Blue, Rgb(0, 0, 238)),
    (Color::Magenta, Rgb(205, 0, 205)),
    (Color::Cyan, Rgb(0, 205, 205)),
    (Color::Gray, Rgb(229, 229, 229)),
    (Color::DarkGray, Rgb(127, 127, 127)),
    (Color::LightRed, Rgb(255, 0, 0)),
    (Color::LightGreen, Rgb(0, 255, 0)),
    (Color::LightYellow, Rgb(255, 255, 0)),
    (Color::LightBlue, Rgb(92, 92, 255)),
    (Color::LightMagenta, Rgb(255, 0, 255)),
    (Color::LightCyan, Rgb(0, 255, 255)),
    (Color::White, Rgb(255, 255, 255)),
];

/// Quantizes to the nearest of the 16 basic ANSI colors. Vivid colors never map
/// to black, white or gray, so player colors keep a hue.
pub fn to_ansi16(rgb: Rgb) -> Color {
    let vivid = rgb.chroma() >= 48;
    let mut best = Color::White;
    let mut best_d = u32::MAX;
    for (color, value) in ANSI16.iter() {
        if vivid && value.chroma() == 0 {
            continue;
        }
        let d = rgb.dist2(*value);
        if d < best_d {
            best_d = d;
            best = *color;
        }
    }
    best
}

/// Converts `rgb` to a ratatui color for `mode`.
pub fn quantize(rgb: Rgb, mode: ColorMode) -> Color {
    match mode {
        ColorMode::TrueColor => rgb.to_color(),
        ColorMode::Ansi256 => Color::Indexed(to_ansi256(rgb)),
        ColorMode::Ansi16 => to_ansi16(rgb),
    }
}

/// Number of palette entries: the human plus [`MAX_BOTS`] bots.
pub const PALETTE_LEN: usize = MAX_BOTS as usize + 1;

/// A complete color theme: map colors, UI accents and the player palette.
///
/// Build one with [`Theme::get`]; it is cheap but not free (the palette is
/// generated), so keep it around instead of rebuilding it every frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Which theme this is.
    pub kind: ThemeKind,
    ocean: Rgb,
    ocean_alt: Rgb,
    neutral: Rgb,
    border: Rgb,
    border_mix_pct: u8,
    highlight: Rgb,
    label_dark: Rgb,
    label_light: Rgb,
    accent: Rgb,
    dim: Rgb,
    warn: Rgb,
    good: Rgb,
    bad: Rgb,
    text: Rgb,
    panel_bg: Rgb,
    players: Vec<Rgb>,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme::get(ThemeKind::Dark)
    }
}

impl Theme {
    /// Builds the theme of the given kind.
    pub fn get(kind: ThemeKind) -> Theme {
        match kind {
            ThemeKind::Dark => Theme::dark(),
            ThemeKind::HighContrast => Theme::high_contrast(),
            ThemeKind::ColorblindSafe => Theme::colorblind_safe(),
        }
    }

    fn dark() -> Theme {
        let hues: [f32; 10] = [
            0.0, 212.0, 125.0, 285.0, 22.0, 175.0, 328.0, 90.0, 250.0, 195.0,
        ];
        // (hue offset, saturation, lightness) per tier.
        let tiers: [(f32, f32, f32); 4] = [
            (0.0, 0.72, 0.55),
            (0.0, 0.62, 0.38),
            (0.0, 0.78, 0.72),
            (16.0, 0.55, 0.50),
        ];
        let players = hue_tier_palette(Rgb(255, 200, 40), &hues, &tiers);
        Theme {
            kind: ThemeKind::Dark,
            ocean: Rgb(13, 27, 48),
            ocean_alt: Rgb(19, 37, 63),
            neutral: Rgb(78, 82, 90),
            border: Rgb::BLACK,
            border_mix_pct: 42,
            highlight: Rgb::WHITE,
            label_dark: Rgb(12, 12, 16),
            label_light: Rgb(245, 245, 245),
            accent: Rgb(95, 200, 255),
            dim: Rgb(120, 128, 140),
            warn: Rgb(255, 190, 60),
            good: Rgb(110, 220, 120),
            bad: Rgb(255, 95, 95),
            text: Rgb(220, 224, 230),
            panel_bg: Rgb(16, 18, 24),
            players,
        }
    }

    fn high_contrast() -> Theme {
        let hues: [f32; 10] = [
            0.0, 120.0, 225.0, 55.0, 180.0, 300.0, 30.0, 270.0, 150.0, 330.0,
        ];
        let tiers: [(f32, f32, f32); 4] = [
            (0.0, 1.0, 0.50),
            (0.0, 1.0, 0.75),
            (0.0, 1.0, 0.33),
            (15.0, 0.9, 0.62),
        ];
        let players = hue_tier_palette(Rgb::WHITE, &hues, &tiers);
        Theme {
            kind: ThemeKind::HighContrast,
            ocean: Rgb(0, 0, 0),
            ocean_alt: Rgb(10, 10, 30),
            neutral: Rgb(128, 128, 128),
            border: Rgb::BLACK,
            border_mix_pct: 70,
            highlight: Rgb(255, 255, 0),
            label_dark: Rgb::BLACK,
            label_light: Rgb::WHITE,
            accent: Rgb(0, 255, 255),
            dim: Rgb(190, 190, 190),
            warn: Rgb(255, 255, 0),
            good: Rgb(0, 255, 0),
            bad: Rgb(255, 60, 60),
            text: Rgb::WHITE,
            panel_bg: Rgb::BLACK,
            players,
        }
    }

    fn colorblind_safe() -> Theme {
        // Okabe-Ito: orange, sky blue, bluish green, blue, vermillion, reddish purple.
        // Yellow is reserved for the human player; black is the map border.
        let base: [Rgb; 6] = [
            Rgb(230, 159, 0),
            Rgb(86, 180, 233),
            Rgb(0, 158, 115),
            Rgb(0, 114, 178),
            Rgb(213, 94, 0),
            Rgb(204, 121, 167),
        ];
        // Lightness variants: positive = toward white, negative = toward black.
        let variants: [i8; 7] = [0, -35, 40, -55, 65, -18, 20];
        let mut players = Vec::with_capacity(PALETTE_LEN);
        players.push(Rgb(240, 228, 66));
        let mut k = 0usize;
        while players.len() < PALETTE_LEN {
            let tier = (k / base.len()) % variants.len();
            let c = base[k % base.len()];
            let v = variants[tier];
            let shaded = if v >= 0 {
                c.lighten(v as u8)
            } else {
                c.darken(v.unsigned_abs())
            };
            players.push(shaded);
            k += 1;
        }
        Theme {
            kind: ThemeKind::ColorblindSafe,
            ocean: Rgb(8, 12, 22),
            ocean_alt: Rgb(14, 20, 34),
            neutral: Rgb(92, 92, 92),
            border: Rgb::BLACK,
            border_mix_pct: 48,
            highlight: Rgb::WHITE,
            label_dark: Rgb(10, 10, 10),
            label_light: Rgb(245, 245, 245),
            accent: Rgb(86, 180, 233),
            dim: Rgb(140, 140, 140),
            warn: Rgb(230, 159, 0),
            good: Rgb(0, 158, 115),
            bad: Rgb(213, 94, 0),
            text: Rgb(230, 230, 230),
            panel_bg: Rgb(14, 14, 18),
            players,
        }
    }

    /// The color of player `p`. Ids beyond the palette wrap around.
    pub fn player_color(&self, p: PlayerId) -> Rgb {
        if self.players.is_empty() {
            return self.neutral;
        }
        self.players[usize::from(p) % self.players.len()]
    }

    /// Converts `rgb` to a ratatui color, quantized for `mode`.
    pub fn color(&self, rgb: Rgb, mode: ColorMode) -> Color {
        quantize(rgb, mode)
    }

    /// Player `p`'s color, ready for ratatui (e.g. for names in the leaderboard or event log).
    pub fn player_fg(&self, p: PlayerId, mode: ColorMode) -> Color {
        quantize(self.player_color(p), mode)
    }

    /// The fill color of a region owned by `owner` (`None` = neutral).
    pub fn owner_color(&self, owner: Option<PlayerId>) -> Rgb {
        match owner {
            Some(p) => self.player_color(p),
            None => self.neutral,
        }
    }

    /// The current fill color of region `r` in `game` (neutral for unknown ids).
    pub fn region_color(&self, game: &Game, r: RegionId) -> Rgb {
        match game.regions().get(usize::from(r)) {
            Some(st) => self.owner_color(st.owner),
            None => self.neutral,
        }
    }

    /// The border shade of a fill color: darker, or lighter for very dark colors.
    pub fn border_of(&self, fill: Rgb) -> Rgb {
        if fill.luminance() < 48 {
            fill.lighten(self.border_mix_pct / 2)
        } else {
            fill.mix(self.border, self.border_mix_pct)
        }
    }

    /// The color a highlighted region pulses toward from `fill`.
    pub fn pulse_target(&self, fill: Rgb) -> Rgb {
        if fill.luminance() > 170 {
            Rgb::BLACK
        } else {
            self.highlight
        }
    }

    /// A readable text color on top of `bg` (dark on light fills, light on dark ones).
    pub fn text_on(&self, bg: Rgb) -> Rgb {
        if bg.luminance() > 140 {
            self.label_dark
        } else {
            self.label_light
        }
    }

    /// Ocean color.
    pub fn ocean(&self) -> Rgb {
        self.ocean
    }

    /// The subtle second ocean color used for texture.
    pub fn ocean_alt(&self) -> Rgb {
        self.ocean_alt
    }

    /// Neutral (unowned) region color.
    pub fn neutral(&self) -> Rgb {
        self.neutral
    }

    /// Highlight color.
    pub fn highlight(&self) -> Rgb {
        self.highlight
    }

    /// UI accent (titles, the prompt, selected items).
    pub fn accent(&self) -> Rgb {
        self.accent
    }

    /// Dimmed UI text (hints, ghost text, secondary info).
    pub fn dim(&self) -> Rgb {
        self.dim
    }

    /// Warnings.
    pub fn warn(&self) -> Rgb {
        self.warn
    }

    /// Good news (captures, successful commands).
    pub fn good(&self) -> Rgb {
        self.good
    }

    /// Bad news (losses, rejected commands).
    pub fn bad(&self) -> Rgb {
        self.bad
    }

    /// Normal UI text.
    pub fn text(&self) -> Rgb {
        self.text
    }

    /// Panel background.
    pub fn panel_bg(&self) -> Rgb {
        self.panel_bg
    }

    /// The whole player palette (index = player id).
    pub fn palette(&self) -> &[Rgb] {
        &self.players
    }
}

/// The current fill color of region `r`: its owner's color, or neutral.
pub fn region_color(theme: &Theme, game: &Game, r: RegionId) -> Rgb {
    theme.region_color(game, r)
}

/// Builds a palette: `human` first, then bots in tier-major order so that small
/// matches get the most distinct colors.
fn hue_tier_palette(human: Rgb, hues: &[f32], tiers: &[(f32, f32, f32)]) -> Vec<Rgb> {
    let mut players = Vec::with_capacity(PALETTE_LEN);
    players.push(human);
    let mut k = 0usize;
    while players.len() < PALETTE_LEN {
        let tier = tiers[(k / hues.len()) % tiers.len()];
        let hue = hues[k % hues.len()] + tier.0;
        players.push(Rgb::from_hsl(hue, tier.1, tier.2));
        k += 1;
    }
    players
}
