//! The match setup form: field list, editing and building the [`MatchConfig`].

use std::time::{SystemTime, UNIX_EPOCH};

use territorial_core::config::DEFAULT_SEED;
use territorial_core::{Difficulty, MatchConfig, Preset, StartChoice, WinCondition, MAX_BOTS};
use territorial_render::{RendererChoice, ThemeKind};

/// The rows of the setup form, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Preset,
    Minutes,
    Bots,
    Difficulty,
    Win,
    DominationPct,
    Start,
    Alliances,
    Seed,
    Theme,
    Renderer,
}

impl Field {
    /// Every field in display order.
    pub const ALL: [Field; 11] = [
        Field::Preset,
        Field::Minutes,
        Field::Bots,
        Field::Difficulty,
        Field::Win,
        Field::DominationPct,
        Field::Start,
        Field::Alliances,
        Field::Seed,
        Field::Theme,
        Field::Renderer,
    ];

    /// Row label.
    pub fn label(self) -> &'static str {
        match self {
            Field::Preset => "Preset",
            Field::Minutes => "Match length",
            Field::Bots => "Bots",
            Field::Difficulty => "Difficulty",
            Field::Win => "Win condition",
            Field::DominationPct => "Domination %",
            Field::Start => "Start region",
            Field::Alliances => "Alliances",
            Field::Seed => "Seed",
            Field::Theme => "Theme",
            Field::Renderer => "Renderer",
        }
    }

    /// One-line explanation shown under the form.
    pub fn help(self) -> &'static str {
        match self {
            Field::Preset => "Blitz 8 min / Standard 25 min / Epic 75 min; Custom uses your own length",
            Field::Minutes => "Type minutes (0 = no timer, not for Timed); changing it switches to Custom",
            Field::Bots => "Number of bot opponents, 3 to 40 (type a number)",
            Field::Difficulty => "Easy reacts slowly; Hard coordinates attacks; Mixed varies per bot",
            Field::Win => "Domination: hold a share of world value; Elimination: last standing; Timed: best score",
            Field::DominationPct => "Share of total world value needed to win by domination (10-100)",
            Field::Start => "Random spaced start, or choose your start region on the map",
            Field::Alliances => "Allow alliances (ally/unally) with bots",
            Field::Seed => "Random, or type digits for a fixed, repeatable seed",
            Field::Theme => "Color theme (saved in your settings)",
            Field::Renderer => "Map renderer (saved in your settings)",
        }
    }
}

/// The three win condition kinds shown in the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinKind {
    Domination,
    Elimination,
    Timed,
}

/// What a key press in the form asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormChange {
    /// Nothing outside the form changed.
    None,
    /// The theme changed (apply and save settings).
    Theme,
    /// The renderer changed (apply and save settings).
    Renderer,
}

/// The setup form state.
#[derive(Clone, Debug)]
pub struct SetupForm {
    pub selected: usize,
    pub preset: Preset,
    pub minutes: u32,
    pub bots: u32,
    pub difficulty: Difficulty,
    pub win: WinKind,
    pub dom_pct: u32,
    pub choose_start: bool,
    pub alliances: bool,
    pub seed_random: bool,
    pub seed: u64,
    pub theme: ThemeKind,
    pub renderer: RendererChoice,
    /// The rest of the config (name, growth, spectate) is carried over from here.
    base: MatchConfig,
    /// True right after a field is selected: the next digit replaces the value.
    fresh: bool,
}

impl SetupForm {
    /// A form prefilled from `cfg`. A seed equal to `DEFAULT_SEED` means "random".
    pub fn from_config(cfg: &MatchConfig, theme: ThemeKind, renderer: RendererChoice) -> SetupForm {
        let (win, dom_pct) = match cfg.win {
            WinCondition::Domination { pct } => (WinKind::Domination, u32::from(pct)),
            WinCondition::Elimination => (WinKind::Elimination, 60),
            WinCondition::Timed => (WinKind::Timed, 60),
        };
        let minutes = match cfg.time_limit_secs {
            Some(s) => (s + 59) / 60,
            None => 0,
        };
        SetupForm {
            selected: 0,
            preset: cfg.preset,
            minutes,
            bots: u32::from(cfg.bots),
            difficulty: cfg.difficulty,
            win,
            dom_pct,
            choose_start: false,
            alliances: cfg.alliances,
            seed_random: cfg.seed == DEFAULT_SEED,
            seed: if cfg.seed == DEFAULT_SEED { 0 } else { cfg.seed },
            theme,
            renderer,
            base: cfg.clone(),
            fresh: true,
        }
    }

    /// The selected field.
    pub fn field(&self) -> Field {
        Field::ALL[self.selected.min(Field::ALL.len() - 1)]
    }

    /// Moves the selection up.
    pub fn up(&mut self) {
        self.clamp_values();
        self.selected = if self.selected == 0 {
            Field::ALL.len() - 1
        } else {
            self.selected - 1
        };
        self.fresh = true;
    }

    /// Moves the selection down.
    pub fn down(&mut self) {
        self.clamp_values();
        self.selected = (self.selected + 1) % Field::ALL.len();
        self.fresh = true;
    }

    /// Changes the selected field by one step (`dir` is -1 or +1).
    pub fn change(&mut self, dir: i32) -> FormChange {
        self.fresh = true;
        match self.field() {
            Field::Preset => {
                let all = Preset::ALL;
                let idx = all.iter().position(|p| *p == self.preset).unwrap_or(1);
                let next = cycle(idx, all.len(), dir);
                self.set_preset(all[next]);
            }
            Field::Minutes => {
                let step: u32 = if self.minutes >= 30 { 5 } else { 1 };
                if dir < 0 {
                    self.minutes = self.minutes.saturating_sub(step);
                } else {
                    self.minutes = (self.minutes + step).min(600);
                }
                self.preset = Preset::Custom;
            }
            Field::Bots => {
                if dir < 0 {
                    self.bots = self.bots.saturating_sub(1).max(3);
                } else {
                    self.bots = (self.bots + 1).min(u32::from(MAX_BOTS));
                }
            }
            Field::Difficulty => {
                let all = Difficulty::ALL;
                let idx = all.iter().position(|d| *d == self.difficulty).unwrap_or(1);
                self.difficulty = all[cycle(idx, all.len(), dir)];
            }
            Field::Win => {
                let all = [WinKind::Domination, WinKind::Elimination, WinKind::Timed];
                let idx = all.iter().position(|w| *w == self.win).unwrap_or(0);
                self.win = all[cycle(idx, all.len(), dir)];
            }
            Field::DominationPct => {
                if dir < 0 {
                    self.dom_pct = self.dom_pct.saturating_sub(5).max(10);
                } else {
                    self.dom_pct = (self.dom_pct + 5).min(100);
                }
            }
            Field::Start => self.choose_start = !self.choose_start,
            Field::Alliances => self.alliances = !self.alliances,
            Field::Seed => self.seed_random = !self.seed_random,
            Field::Theme => {
                let all = ThemeKind::ALL;
                let idx = all.iter().position(|t| *t == self.theme).unwrap_or(0);
                self.theme = all[cycle(idx, all.len(), dir)];
                return FormChange::Theme;
            }
            Field::Renderer => {
                let all = RendererChoice::ALL;
                let idx = all.iter().position(|r| *r == self.renderer).unwrap_or(0);
                self.renderer = all[cycle(idx, all.len(), dir)];
                return FormChange::Renderer;
            }
        }
        FormChange::None
    }

    /// Types a digit into a numeric field. Returns false if the field is not numeric.
    pub fn digit(&mut self, d: u32) -> bool {
        let fresh = self.fresh;
        self.fresh = false;
        match self.field() {
            Field::Minutes => {
                self.minutes = push_digit(self.minutes, d, fresh, 600);
                self.preset = Preset::Custom;
                true
            }
            Field::Bots => {
                self.bots = push_digit(self.bots, d, fresh, u32::from(MAX_BOTS));
                true
            }
            Field::DominationPct => {
                self.dom_pct = push_digit(self.dom_pct, d, fresh, 100);
                true
            }
            Field::Seed => {
                let base = if fresh || self.seed_random { 0 } else { self.seed };
                self.seed = match base.checked_mul(10).and_then(|v| v.checked_add(u64::from(d))) {
                    Some(v) => v,
                    None => self.seed,
                };
                self.seed_random = false;
                true
            }
            _ => false,
        }
    }

    /// Deletes the last digit of a numeric field.
    pub fn backspace(&mut self) {
        self.fresh = false;
        match self.field() {
            Field::Minutes => {
                self.minutes /= 10;
                self.preset = Preset::Custom;
            }
            Field::Bots => self.bots /= 10,
            Field::DominationPct => self.dom_pct /= 10,
            Field::Seed => {
                if !self.seed_random {
                    self.seed /= 10;
                }
            }
            _ => {}
        }
    }

    /// The display value of a field.
    pub fn value_text(&self, f: Field) -> String {
        match f {
            Field::Preset => capitalize(self.preset.name()),
            Field::Minutes => {
                if self.minutes == 0 {
                    "no timer".to_string()
                } else {
                    format!("{} min", self.minutes)
                }
            }
            Field::Bots => self.bots.to_string(),
            Field::Difficulty => capitalize(self.difficulty.name()),
            Field::Win => match self.win {
                WinKind::Domination => "Domination".to_string(),
                WinKind::Elimination => "Elimination".to_string(),
                WinKind::Timed => "Timed".to_string(),
            },
            Field::DominationPct => {
                if self.win == WinKind::Domination {
                    format!("{}%", self.dom_pct)
                } else {
                    format!("{}% (unused)", self.dom_pct)
                }
            }
            Field::Start => {
                if self.choose_start {
                    "Choose on map".to_string()
                } else {
                    "Random".to_string()
                }
            }
            Field::Alliances => {
                if self.alliances {
                    "On".to_string()
                } else {
                    "Off".to_string()
                }
            }
            Field::Seed => {
                if self.seed_random {
                    "Random".to_string()
                } else {
                    format!("Fixed: {}", self.seed)
                }
            }
            Field::Theme => self.theme.label().to_string(),
            Field::Renderer => self.renderer.name().to_string(),
        }
    }

    /// Whether a field holds a number that can be typed.
    pub fn is_numeric(f: Field) -> bool {
        matches!(
            f,
            Field::Minutes | Field::Bots | Field::DominationPct | Field::Seed
        )
    }

    /// Builds the match configuration. A random seed is rolled here.
    /// Also returns the config to remember in settings (random seed stored as `DEFAULT_SEED`).
    pub fn build(&mut self) -> (MatchConfig, MatchConfig) {
        self.clamp_values();
        let mut cfg = MatchConfig::preset(self.preset);
        cfg.human_name = self.base.human_name.clone();
        cfg.spectate = self.base.spectate;
        if self.preset == Preset::Custom {
            if self.minutes == 0 {
                cfg.time_limit_secs = None;
            } else {
                cfg = cfg.with_custom_minutes(self.minutes);
            }
        } else if self.minutes == 0 {
            cfg.time_limit_secs = None;
        }
        cfg.bots = self.bots as u8;
        cfg.difficulty = self.difficulty;
        cfg.win = match self.win {
            WinKind::Domination => WinCondition::Domination {
                pct: self.dom_pct as u8,
            },
            WinKind::Elimination => WinCondition::Elimination,
            WinKind::Timed => WinCondition::Timed,
        };
        cfg.start = StartChoice::Random;
        cfg.alliances = self.alliances;
        let mut remembered = cfg.clone();
        if self.seed_random {
            cfg.seed = random_seed();
            remembered.seed = DEFAULT_SEED;
        } else {
            cfg.seed = self.seed;
            remembered.seed = self.seed;
        }
        (cfg, remembered)
    }

    fn set_preset(&mut self, p: Preset) {
        self.preset = p;
        if p != Preset::Custom {
            let cfg = MatchConfig::preset(p);
            self.minutes = cfg.time_limit_secs.map(|s| s / 60).unwrap_or(0);
            self.bots = u32::from(cfg.bots);
        }
    }

    fn clamp_values(&mut self) {
        self.bots = self.bots.clamp(3, u32::from(MAX_BOTS));
        self.dom_pct = self.dom_pct.clamp(10, 100);
        self.minutes = self.minutes.min(600);
    }
}

fn cycle(idx: usize, len: usize, dir: i32) -> usize {
    if len == 0 {
        return 0;
    }
    if dir < 0 {
        (idx + len - 1) % len
    } else {
        (idx + 1) % len
    }
}

fn push_digit(value: u32, d: u32, fresh: bool, max: u32) -> u32 {
    if fresh {
        return d;
    }
    let v = value.saturating_mul(10).saturating_add(d);
    if v > max {
        d
    } else {
        v
    }
}

fn capitalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut first = true;
    for c in s.chars() {
        if first {
            out.extend(c.to_uppercase());
            first = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// A fresh random seed from the clock (UI only; the simulation itself stays
/// deterministic for a given seed).
pub fn random_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x1234_5678);
    let mut z = nanos.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
