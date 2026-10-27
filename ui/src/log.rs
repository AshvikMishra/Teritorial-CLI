//! The event log: styled entries built from simulation events and command
//! echoes. Entries keep semantic styles (not colors), so a theme switch
//! recolors the whole log.

use std::time::{Duration, Instant};

use territorial_core::{
    Command, EventKind, Game, GameEvent, Outcome, PlayerId, RegionId, WinReason,
};
use territorial_render::art;

use crate::command::{fmt_num, fmt_ticks};

/// Maximum entries kept in the log.
pub const LOG_CAPACITY: usize = 600;
/// How long a fresh entry flashes.
pub const FLASH_TIME: Duration = Duration::from_millis(1500);

/// The semantic style of a log segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Normal text.
    Text,
    /// Secondary text.
    Dim,
    /// Accent color (verbs, highlights).
    Accent,
    /// Good news.
    Good,
    /// Bad news.
    Bad,
    /// Warnings (incoming attacks).
    Warn,
    /// A player's name, in their color.
    Player(PlayerId),
}

/// One run of text with one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    /// The text.
    pub text: String,
    /// Its style.
    pub tone: Tone,
}

/// One line in the event log.
#[derive(Clone, Debug)]
pub struct LogEntry {
    /// Game tick the entry was made at (shown as a clock).
    pub tick: u64,
    /// The styled text.
    pub segs: Vec<Seg>,
    /// Concerns the human player directly (drawn bold).
    pub important: bool,
    /// Flash (reverse video) until this instant.
    pub flash_until: Option<Instant>,
}

impl LogEntry {
    /// Whether the entry is flashing at `now`.
    pub fn flashing(&self, now: Instant) -> bool {
        match self.flash_until {
            Some(t) => now < t,
            None => false,
        }
    }
}

/// Builds the segments of one entry.
#[derive(Clone, Debug, Default)]
pub struct Msg {
    segs: Vec<Seg>,
}

impl Msg {
    /// An empty line.
    pub fn new() -> Msg {
        Msg { segs: Vec::new() }
    }

    /// Appends text with a tone.
    pub fn push(mut self, text: &str, tone: Tone) -> Msg {
        self.segs.push(Seg {
            text: text.to_string(),
            tone,
        });
        self
    }

    /// Appends normal text.
    pub fn text(self, text: &str) -> Msg {
        self.push(text, Tone::Text)
    }

    /// Appends a player's name in their color.
    pub fn player(self, game: &Game, p: PlayerId) -> Msg {
        let name = player_name(game, p);
        self.push(&name, Tone::Player(p))
    }

    /// Appends a region's name.
    pub fn region(self, game: &Game, r: RegionId) -> Msg {
        let name = region_name(game, r);
        self.push(&name, Tone::Accent)
    }

    /// The finished segments.
    pub fn into_segs(self) -> Vec<Seg> {
        self.segs
    }
}

/// The scrollable event log.
#[derive(Clone, Debug, Default)]
pub struct EventLog {
    entries: Vec<LogEntry>,
    scroll: usize,
}

impl EventLog {
    /// An empty log.
    pub fn new() -> EventLog {
        EventLog {
            entries: Vec::new(),
            scroll: 0,
        }
    }

    /// Removes every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.scroll = 0;
    }

    /// Adds an entry. When the view is scrolled up it stays where it is.
    pub fn push(&mut self, tick: u64, line: Msg, important: bool, flash: bool, now: Instant) {
        let entry = LogEntry {
            tick,
            segs: line.into_segs(),
            important,
            flash_until: if flash { Some(now + FLASH_TIME) } else { None },
        };
        self.entries.push(entry);
        if self.scroll > 0 {
            self.scroll += 1;
        }
        if self.entries.len() > LOG_CAPACITY {
            let excess = self.entries.len() - LOG_CAPACITY;
            self.entries = self.entries.split_off(excess);
        }
        self.clamp_scroll();
    }

    /// Adds a plain message in one tone.
    pub fn message(&mut self, tick: u64, text: &str, tone: Tone, now: Instant) {
        self.push(tick, Msg::new().push(text, tone), false, false, now);
    }

    /// All entries, oldest first.
    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    /// How many lines the view is scrolled up from the newest entry.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Scrolls towards older entries.
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
        self.clamp_scroll();
    }

    /// Scrolls towards newer entries.
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    /// Whether any entry is still flashing.
    pub fn any_flashing(&self, now: Instant) -> bool {
        self.entries.iter().rev().take(32).any(|e| e.flashing(now))
    }

    fn clamp_scroll(&mut self) {
        let max = self.entries.len().saturating_sub(1);
        if self.scroll > max {
            self.scroll = max;
        }
    }
}

/// A player's display name.
pub fn player_name(game: &Game, p: PlayerId) -> String {
    match game.get_player(p) {
        Some(pl) => pl.name.clone(),
        None => format!("Player {}", p),
    }
}

/// A region's display name.
pub fn region_name(game: &Game, r: RegionId) -> String {
    match game.map().get_region(r) {
        Some(info) => info.name.clone(),
        None => format!("region {}", r),
    }
}

/// Formats seconds as `m:ss` or `h:mm:ss`.
pub fn clock(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

/// A short phrase for a game command, e.g. `attack Brazil`.
fn command_phrase(game: &Game, cmd: &Command) -> String {
    match *cmd {
        Command::Attack { target, .. } => format!("attack {}", region_name(game, target)),
        Command::Fortify { region, .. } => format!("fortify {}", region_name(game, region)),
        Command::Recall { region, .. } => format!("recall from {}", region_name(game, region)),
        Command::ProposeAlliance { with } => format!("ally {}", player_name(game, with)),
        Command::BreakAlliance { with } => format!("unally {}", player_name(game, with)),
    }
}

/// The headline for an outcome, e.g. `Bot-3 wins by domination`.
pub fn outcome_text(game: &Game, outcome: &Outcome) -> String {
    match outcome {
        Outcome::Winner { player, reason } => {
            let why = match reason {
                WinReason::Domination => "by domination",
                WinReason::Elimination => "by elimination",
                WinReason::Timer => "on time",
            };
            format!("{} wins {}", player_name(game, *player), why)
        }
        Outcome::AlliedVictory { players, .. } => {
            let names: Vec<String> = players.iter().map(|&p| player_name(game, p)).collect();
            format!("Allied victory: {}", names.join(", "))
        }
    }
}

/// What the log should show for one simulation event, as
/// `(line, important, flash)`, or `None` for events not worth a line
/// (most bot-only activity).
pub fn describe_event(game: &Game, ev: &GameEvent, viewer: PlayerId) -> Option<(Msg, bool, bool)> {
    let me = viewer;
    match &ev.kind {
        EventKind::CommandApplied { .. } => None,
        EventKind::CommandRejected {
            player,
            command,
            reason,
        } => {
            if *player != me {
                return None;
            }
            let line = Msg::new()
                .push("rejected: ", Tone::Bad)
                .text(&command_phrase(game, command))
                .push(" - ", Tone::Dim)
                .push(&reason.to_string(), Tone::Bad);
            Some((line, true, false))
        }
        EventKind::MarchStarted {
            player,
            from,
            to,
            troops,
            eta_ticks,
            ..
        } => {
            let target_owner = game.regions().get(*to as usize).and_then(|s| s.owner);
            if *player == me {
                let line = Msg::new()
                    .push(&fmt_num(*troops), Tone::Good)
                    .text(" troops march from ")
                    .region(game, *from)
                    .text(" to ")
                    .region(game, *to)
                    .push(&format!("  ETA {}", fmt_ticks(*eta_ticks)), Tone::Dim);
                Some((line, false, false))
            } else if target_owner == Some(me) {
                let line = Msg::new()
                    .push("! ", Tone::Warn)
                    .player(game, *player)
                    .push(" marches ", Tone::Warn)
                    .push(&fmt_num(*troops), Tone::Warn)
                    .push(" troops on ", Tone::Warn)
                    .region(game, *to)
                    .push(&format!("  ETA {}", fmt_ticks(*eta_ticks)), Tone::Warn);
                Some((line, true, true))
            } else {
                None
            }
        }
        EventKind::Captured {
            region,
            by,
            from,
            survivors,
        } => {
            let flourish = art::capture_flourish(u32::from(*region).wrapping_add(ev.tick as u32));
            if *by == me {
                let mut line = Msg::new()
                    .push(flourish, Tone::Good)
                    .text(" ")
                    .push("You took ", Tone::Good)
                    .region(game, *region);
                if let Some(prev) = from {
                    line = line.text(" from ").player(game, *prev);
                }
                line = line.push(
                    &format!("  ({} survivors)", fmt_num(*survivors)),
                    Tone::Dim,
                );
                Some((line, true, true))
            } else if *from == Some(me) {
                let line = Msg::new()
                    .push("x ", Tone::Bad)
                    .player(game, *by)
                    .push(" captured your ", Tone::Bad)
                    .region(game, *region);
                Some((line, true, true))
            } else if let Some(prev) = from {
                let line = Msg::new()
                    .push(flourish, Tone::Dim)
                    .text(" ")
                    .player(game, *by)
                    .text(" captured ")
                    .region(game, *region)
                    .text(" from ")
                    .player(game, *prev);
                Some((line, false, false))
            } else {
                None
            }
        }
        EventKind::AttackRepelled {
            region,
            attacker,
            defender,
            attacker_lost,
            defender_lost,
        } => {
            if *attacker == me {
                let line = Msg::new()
                    .push("Attack on ", Tone::Bad)
                    .region(game, *region)
                    .push(" failed", Tone::Bad)
                    .push(
                        &format!(
                            "  (lost {}, killed {})",
                            fmt_num(*attacker_lost),
                            fmt_num(*defender_lost)
                        ),
                        Tone::Dim,
                    );
                Some((line, true, false))
            } else if *defender == Some(me) {
                let line = Msg::new()
                    .region(game, *region)
                    .push(" held against ", Tone::Good)
                    .player(game, *attacker)
                    .push(
                        &format!(
                            "  (lost {}, killed {})",
                            fmt_num(*defender_lost),
                            fmt_num(*attacker_lost)
                        ),
                        Tone::Dim,
                    );
                Some((line, true, false))
            } else {
                None
            }
        }
        EventKind::Reinforced {
            region,
            player,
            troops,
        } => {
            if *player != me {
                return None;
            }
            let line = Msg::new()
                .push(&format!("+{} troops reinforced ", fmt_num(*troops)), Tone::Dim)
                .region(game, *region);
            Some((line, false, false))
        }
        EventKind::RecallArrived { player, troops } => {
            if *player != me {
                return None;
            }
            let line = Msg::new().push(
                &format!("+{} troops returned to the pool", fmt_num(*troops)),
                Tone::Dim,
            );
            Some((line, false, false))
        }
        EventKind::Eliminated { player, by } => {
            let flourish = art::elimination_flourish(u32::from(*player).wrapping_add(ev.tick as u32));
            let tone = if *player == me { Tone::Bad } else { Tone::Accent };
            let mut line = Msg::new().push(flourish, tone).text(" ");
            if *player == me {
                line = line.push("You have been eliminated", Tone::Bad);
            } else {
                line = line.player(game, *player).push(" has been eliminated", tone);
            }
            if let Some(b) = by {
                line = line.text(" by ").player(game, *b);
            }
            let important = *player == me || *by == Some(me);
            Some((line, important, important))
        }
        EventKind::AllianceProposed { from, to } => {
            if *to == me {
                let name = player_name(game, *from);
                let line = Msg::new()
                    .player(game, *from)
                    .push(" proposes an alliance - type ", Tone::Accent)
                    .push(&format!("ally {}", name.to_lowercase()), Tone::Good)
                    .push(" to accept", Tone::Accent);
                Some((line, true, true))
            } else if *from == me {
                let line = Msg::new()
                    .push("Alliance proposed to ", Tone::Dim)
                    .player(game, *to);
                Some((line, false, false))
            } else {
                None
            }
        }
        EventKind::AllianceFormed { a, b } => {
            let mine = *a == me || *b == me;
            let line = if mine {
                let other = if *a == me { *b } else { *a };
                Msg::new()
                    .push("Alliance formed with ", Tone::Good)
                    .player(game, other)
            } else {
                Msg::new()
                    .player(game, *a)
                    .push(" and ", Tone::Dim)
                    .player(game, *b)
                    .push(" are now allies", Tone::Dim)
            };
            Some((line, mine, mine))
        }
        EventKind::AllianceBroken { by, with } => {
            if *with == me {
                let line = Msg::new()
                    .player(game, *by)
                    .push(" broke your alliance (truce in effect)", Tone::Bad);
                Some((line, true, true))
            } else if *by == me {
                let line = Msg::new()
                    .push("You broke the alliance with ", Tone::Warn)
                    .player(game, *with);
                Some((line, true, false))
            } else {
                let line = Msg::new()
                    .player(game, *by)
                    .push(" broke the alliance with ", Tone::Dim)
                    .player(game, *with);
                Some((line, false, false))
            }
        }
        EventKind::ProposalExpired { from, to } => {
            if *from == me {
                let line = Msg::new()
                    .push("Your alliance proposal to ", Tone::Dim)
                    .player(game, *to)
                    .push(" expired", Tone::Dim);
                Some((line, false, false))
            } else if *to == me {
                let line = Msg::new()
                    .push("The alliance proposal from ", Tone::Dim)
                    .player(game, *from)
                    .push(" expired", Tone::Dim);
                Some((line, false, false))
            } else {
                None
            }
        }
        EventKind::MatchOver { outcome } => {
            let won = outcome.is_winner(me);
            let tone = if won { Tone::Good } else { Tone::Accent };
            let line = Msg::new()
                .push("MATCH OVER: ", tone)
                .push(&outcome_text(game, outcome), tone);
            Some((line, true, true))
        }
    }
}
