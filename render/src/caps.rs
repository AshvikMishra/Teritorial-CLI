//! Terminal capability detection and the user's renderer choice.
//!
//! Detection only looks at environment variables (`COLORTERM`, `TERM`,
//! `TERM_PROGRAM`, `WT_SESSION`, `LC_ALL`, `LC_CTYPE`, `LANG`). It never queries the
//! terminal, so it is instant and safe to call before raw mode is enabled.

use serde::{Deserialize, Serialize};

/// How many colors the terminal can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    /// 24-bit RGB colors.
    TrueColor,
    /// The xterm 256-color palette.
    Ansi256,
    /// The 16 basic ANSI colors.
    Ansi16,
}

impl ColorMode {
    /// A short lowercase name, e.g. `"truecolor"`.
    pub fn name(self) -> &'static str {
        match self {
            ColorMode::TrueColor => "truecolor",
            ColorMode::Ansi256 => "256-color",
            ColorMode::Ansi16 => "16-color",
        }
    }
}

/// Which glyphs the terminal can draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GlyphMode {
    /// Unicode is available: the half-block map renderer and arrow glyphs are used.
    Unicode,
    /// ASCII only: the `#`, `+`, `.` map renderer is used.
    Ascii,
}

impl GlyphMode {
    /// A short lowercase name, e.g. `"unicode"`.
    pub fn name(self) -> &'static str {
        match self {
            GlyphMode::Unicode => "unicode",
            GlyphMode::Ascii => "ascii",
        }
    }
}

/// What the terminal can do. Decides which map renderer runs and how colors are quantized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Capabilities {
    /// Available colors.
    pub color: ColorMode,
    /// Available glyphs.
    pub glyphs: GlyphMode,
}

impl Default for Capabilities {
    fn default() -> Capabilities {
        Capabilities {
            color: ColorMode::TrueColor,
            glyphs: GlyphMode::Unicode,
        }
    }
}

/// The renderer the user asked for in settings or with `--renderer`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RendererChoice {
    /// Use whatever [`Capabilities::detect`] found.
    #[default]
    Auto,
    /// Force the truecolor half-block renderer.
    TrueColor,
    /// Force the 256-color half-block renderer.
    Ansi256,
    /// Force the ASCII renderer with 16 colors.
    Ascii,
}

impl RendererChoice {
    /// Every choice, in menu order.
    pub const ALL: [RendererChoice; 4] = [
        RendererChoice::Auto,
        RendererChoice::TrueColor,
        RendererChoice::Ansi256,
        RendererChoice::Ascii,
    ];

    /// The canonical lowercase name (`auto`, `truecolor`, `ansi256`, `ascii`).
    /// Matches the serde representation.
    pub fn name(self) -> &'static str {
        match self {
            RendererChoice::Auto => "auto",
            RendererChoice::TrueColor => "truecolor",
            RendererChoice::Ansi256 => "ansi256",
            RendererChoice::Ascii => "ascii",
        }
    }

    /// Parses a name, case-insensitively. Accepts the canonical names plus a few
    /// spellings such as `true-color`, `24bit`, `256`, `256color` and `16`.
    pub fn from_name(s: &str) -> Option<RendererChoice> {
        let lower = s.trim().to_ascii_lowercase();
        match lower.as_str() {
            "auto" | "detect" | "default" => Some(RendererChoice::Auto),
            "truecolor" | "true-color" | "true_color" | "24bit" | "24-bit" | "rgb" => {
                Some(RendererChoice::TrueColor)
            }
            "ansi256" | "ansi-256" | "256" | "256color" | "256-color" | "xterm256" => {
                Some(RendererChoice::Ansi256)
            }
            "ascii" | "ansi16" | "16" | "16color" | "16-color" | "basic" => {
                Some(RendererChoice::Ascii)
            }
            _ => None,
        }
    }

    /// The next choice in [`RendererChoice::ALL`], wrapping around (for settings menus).
    pub fn next(self) -> RendererChoice {
        match self {
            RendererChoice::Auto => RendererChoice::TrueColor,
            RendererChoice::TrueColor => RendererChoice::Ansi256,
            RendererChoice::Ansi256 => RendererChoice::Ascii,
            RendererChoice::Ascii => RendererChoice::Auto,
        }
    }
}

impl Capabilities {
    /// Detects capabilities from the process environment.
    pub fn detect() -> Capabilities {
        Capabilities::detect_from(|name| std::env::var(name).ok(), cfg!(windows))
    }

    /// Detection with an injectable environment lookup. `var(name)` returns the
    /// variable's value, and `windows` says whether the host is Windows.
    ///
    /// Color rules, first match wins:
    /// `TERM=dumb` gives 16 colors; `COLORTERM` containing `truecolor` or `24bit`,
    /// a `TERM` containing `direct`/`truecolor`/`24bit`, `WT_SESSION` (Windows Terminal) or a
    /// known truecolor `TERM_PROGRAM` gives truecolor; a `TERM` containing `256color`
    /// gives 256 colors; Windows without a `TERM` gives 256 colors; anything else gives 16.
    ///
    /// Glyph rules: `TERM=dumb` gives ASCII. Otherwise the first non-empty of
    /// `LC_ALL`, `LC_CTYPE`, `LANG` decides (UTF-8 means Unicode). With none set,
    /// Windows, `WT_SESSION`, `COLORTERM` or `TERM_PROGRAM` mean Unicode, else ASCII.
    pub fn detect_from<F>(var: F, windows: bool) -> Capabilities
    where
        F: Fn(&str) -> Option<String>,
    {
        let get = |name: &str| -> String {
            match var(name) {
                Some(v) => v.trim().to_ascii_lowercase(),
                None => String::new(),
            }
        };
        let term = get("TERM");
        let colorterm = get("COLORTERM");
        let term_program = get("TERM_PROGRAM");
        let wt_session = !get("WT_SESSION").is_empty();
        let dumb = term == "dumb";

        let truecolor_program = term_program == "iterm.app"
            || term_program == "wezterm"
            || term_program == "vscode"
            || term_program == "hyper"
            || term_program == "ghostty"
            || term_program == "rio";

        let color = if dumb {
            ColorMode::Ansi16
        } else if colorterm.contains("truecolor")
            || colorterm.contains("24bit")
            || term.contains("direct")
            || term.contains("truecolor")
            || term.contains("24bit")
            || wt_session
            || truecolor_program
        {
            ColorMode::TrueColor
        } else if term.contains("256color")
            || term.contains("256-color")
            || (windows && term.is_empty())
        {
            ColorMode::Ansi256
        } else {
            ColorMode::Ansi16
        };

        let mut locale = String::new();
        for name in ["LC_ALL", "LC_CTYPE", "LANG"] {
            let v = get(name);
            if !v.is_empty() {
                locale = v;
                break;
            }
        }
        let glyphs = if dumb {
            GlyphMode::Ascii
        } else if !locale.is_empty() {
            if locale.contains("utf-8") || locale.contains("utf8") || windows || wt_session {
                GlyphMode::Unicode
            } else {
                GlyphMode::Ascii
            }
        } else if windows || wt_session || !colorterm.is_empty() || !term_program.is_empty() {
            GlyphMode::Unicode
        } else {
            GlyphMode::Ascii
        };

        Capabilities { color, glyphs }
    }

    /// Applies the user's renderer choice on top of detected capabilities.
    ///
    /// `Auto` keeps `self`. `TrueColor` and `Ansi256` force that color mode with
    /// Unicode half-blocks. `Ascii` forces ASCII glyphs with 16 colors.
    pub fn with_choice(self, c: RendererChoice) -> Capabilities {
        match c {
            RendererChoice::Auto => self,
            RendererChoice::TrueColor => Capabilities {
                color: ColorMode::TrueColor,
                glyphs: GlyphMode::Unicode,
            },
            RendererChoice::Ansi256 => Capabilities {
                color: ColorMode::Ansi256,
                glyphs: GlyphMode::Unicode,
            },
            RendererChoice::Ascii => Capabilities {
                color: ColorMode::Ansi16,
                glyphs: GlyphMode::Ascii,
            },
        }
    }

    /// Whether the Unicode half-block renderer is used.
    pub fn unicode(&self) -> bool {
        self.glyphs == GlyphMode::Unicode
    }

    /// A short description such as `"truecolor, unicode"`.
    pub fn describe(&self) -> String {
        format!("{}, {}", self.color.name(), self.glyphs.name())
    }
}
