//! The application: screens, the main loop, input handling and the glue
//! between the command system, the simulation and the views.
//!
//! Main loop: wait for input with `recv_timeout` (so an idle game sleeps),
//! advance the simulation with a fixed 10 Hz tick scaled by the speed setting
//! (an `Instant` accumulator), and redraw only when something changed or an
//! animation is running, at most ~60 frames per second.
//!
//! Input while playing has two modes. Hotkey mode (the default) acts at once
//! on the region under the map crosshair (the camera center): `a` attacks it,
//! `Tab` jumps to targets, `1`..`0` set the amount, and so on. `:` or Enter
//! opens prompt mode, the typed command line, which returns to hotkey mode
//! after a successful command or on Esc with an empty line. Hotkey game
//! commands go through the same validation and queue path as typed ones.

use std::cmp::Reverse;
use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use territorial_core::config::DEFAULT_SEED;
use territorial_core::{
    resolve_target, Amount, Balance, Command, EventKind, Game, GameEvent, MatchConfig, Replay,
    ReplayPlayer, RegionId, SmartTarget, StartChoice, WorldMap, HUMAN, TICKS_PER_SECOND,
};
use territorial_render::{Camera, Capabilities, MapRenderer, Theme, ThemeKind, ZoomLevel};

use crate::command::fuzzy::{self, Relevance};
use crate::command::parser::{describe_command, fmt_speed, SPEED_MAX_PCT, SPEED_MIN_PCT};
use crate::command::{self, help, Action, Analysis, CommandContext, History, SubmitResult};
use crate::log::{self, EventLog, Msg, Tone};
use crate::prompt::Prompt;
use crate::settings::{Persistence, Settings, StartMode, UiOptions};
use crate::setup::{Field, FormChange, SetupForm};
use crate::term::{InputThread, TerminalGuard};
use crate::view;

/// Real-time length of one simulation tick at 1x speed, in microseconds.
const TICK_US: u64 = 1_000_000 / TICKS_PER_SECOND;
/// Never run more than this many ticks in one update (avoids a death spiral).
const MAX_STEPS_PER_UPDATE: u32 = 40;
/// Minimum time between two frames (~60 FPS cap).
const FRAME_MIN: Duration = Duration::from_millis(16);
/// Animation step (the map highlight pulse, flashes, splash, countdown).
pub const ANIM_STEP: Duration = Duration::from_millis(40);
/// Time per splash frame.
pub const SPLASH_FRAME: Duration = Duration::from_millis(300);
/// The splash advances on its own after this long.
const SPLASH_TOTAL: Duration = Duration::from_millis(2600);
/// Time per countdown step (3, 2, 1, GO).
pub const COUNT_STEP: Duration = Duration::from_millis(700);
/// How long a focused region keeps pulsing.
const FOCUS_FLASH: Duration = Duration::from_millis(1600);
/// Longest idle wait.
const IDLE_WAIT: Duration = Duration::from_millis(500);
/// Quick-save name.
const QUICK_SAVE: &str = "quicksave";
/// The game speeds `[` and `]` step through, in percent.
const SPEED_STEPS: [u32; 6] = [25, 50, 100, 200, 400, 800];
/// One arrow-key pan in screen cells (horizontal, vertical). Shift or Ctrl pans 4x.
const PAN_STEP: (i32, i32) = (8, 3);
/// Over the ocean the crosshair selects the region whose label is nearest
/// within this many screen pixels (at least this many raster cells).
const SNAP_RADIUS_PX: f32 = 6.0;

/// Which screen is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Splash,
    Setup,
    ChooseStart,
    Countdown,
    Playing,
    GameOver,
}

/// The side panel content (region info, status, aliases).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Panel {
    Region(RegionId),
    Status,
    Aliases,
}

/// The help overlay.
#[derive(Clone, Debug)]
pub struct HelpOverlay {
    pub lines: Vec<String>,
    pub scroll: usize,
}

/// Where the match comes from.
pub enum Driver {
    Live(Box<Game>),
    Replay(Box<ReplayPlayer>),
}

/// The match being played or watched.
pub struct Session {
    pub driver: Driver,
    /// The human was eliminated while the match went on.
    pub human_dead: bool,
    /// Watching the bots finish after being eliminated.
    pub spectating: bool,
    /// The player left with `quit`.
    pub abandoned: bool,
    /// Result of the replay auto-save (path or error).
    pub replay_path: Option<Result<String, String>>,
    /// The replay was already auto-saved (it is saved once per match).
    replay_saved: bool,
}

impl Session {
    fn new(driver: Driver) -> Session {
        Session {
            driver,
            human_dead: false,
            spectating: false,
            abandoned: false,
            replay_path: None,
            replay_saved: false,
        }
    }

    /// The game being shown.
    pub fn game(&self) -> &Game {
        match &self.driver {
            Driver::Live(g) => g,
            Driver::Replay(r) => r.game(),
        }
    }

    /// Whether this is a replay.
    pub fn is_replay(&self) -> bool {
        matches!(self.driver, Driver::Replay(_))
    }

    fn replay_finished(&self) -> bool {
        match &self.driver {
            Driver::Live(_) => false,
            Driver::Replay(r) => r.finished(),
        }
    }

    fn step(&mut self) -> Vec<GameEvent> {
        match &mut self.driver {
            Driver::Live(g) => g.step(),
            Driver::Replay(r) => {
                if r.finished() {
                    Vec::new()
                } else {
                    r.step()
                }
            }
        }
    }

    /// Whether the prompt only accepts view commands.
    pub fn read_only(&self) -> bool {
        self.is_replay() || self.spectating || self.human_dead
    }
}

/// What `.` repeats.
#[derive(Clone, Debug)]
enum Repeat {
    /// A hotkey game command, with the target it had.
    Command(Command),
    /// A command line submitted at the prompt (`!!` expanded).
    Line(String),
}

/// The start-region picker state.
pub struct ChooseState {
    pub config: MatchConfig,
    /// A one-player preview of the match showing where you would start.
    pub preview: Game,
    pub resolved: Option<RegionId>,
    pub error: Option<String>,
}

/// The whole UI state.
pub struct App {
    pub persistence: Box<dyn Persistence>,
    pub settings: Settings,
    pub base_caps: Capabilities,
    pub caps: Capabilities,
    pub theme: Theme,
    pub map: Arc<WorldMap>,
    pub balance: Balance,
    pub renderer: MapRenderer,
    pub camera: Camera,
    pub screen: Screen,
    pub screen_since: Instant,
    pending_start: Option<StartMode>,
    pub form: SetupForm,
    pub setup_msg: Option<(String, bool)>,
    pub saves: Vec<String>,
    pub session: Option<Session>,
    pub choose: Option<ChooseState>,
    pub prompt: Prompt,
    /// Prompt mode (the typed command line) instead of hotkey mode.
    pub prompt_open: bool,
    pub history: History,
    pub analysis: Analysis,
    pub prompt_target: Option<RegionId>,
    pub submit_error: Option<String>,
    pub log: EventLog,
    pub panel: Option<Panel>,
    pub help: Option<HelpOverlay>,
    pub speed_pct: u32,
    pub paused: bool,
    /// The amount hotkey attack, fortify and recall send, in percent (10..=100).
    pub send_pct: u8,
    acc_us: u64,
    last_update: Instant,
    pub anim_frame: u32,
    last_anim: Instant,
    anim_active: bool,
    /// Where the Tab cycle (targets, or start regions) last stopped: region and index.
    tab_cycle: Option<(RegionId, usize)>,
    /// Where the `o` cycle through owned regions last stopped.
    own_cycle: Option<(RegionId, usize)>,
    last_action: Option<Repeat>,
    pub focus_flash: Option<(RegionId, Instant)>,
    dirty: bool,
    quit: bool,
}

/// The game of a session, borrowing only the `session` field.
pub fn session_game(session: &Option<Session>) -> Option<&Game> {
    match session {
        Some(s) => Some(s.game()),
        None => None,
    }
}

/// Builds the command context from the settings and history.
fn make_ctx<'a>(game: &'a Game, settings: &'a Settings, history: &'a History) -> CommandContext<'a> {
    let mut ctx = CommandContext::new(game, HUMAN, &settings.aliases);
    ctx.default_attack_pct_neutral = settings.default_attack_pct_neutral.clamp(1, 100);
    ctx.default_attack_pct_enemy = settings.default_attack_pct_enemy.clamp(1, 100);
    ctx.last_line = history.last();
    ctx
}

/// The amount hotkeys start a match with: the enemy default attack percent
/// rounded to the nearest 10 (10..=100).
fn initial_send_pct(settings: &Settings) -> u8 {
    let p = settings.default_attack_pct_enemy.clamp(1, 100);
    ((p + 5) / 10 * 10).clamp(10, 100)
}

/// Why `what` (a verb) is refused in a read-only session.
fn read_only_error(what: &str) -> String {
    format!(
        "read-only: {} is not available (speed, pause, focus, info, status, help, theme, quit)",
        what
    )
}

/// The region under the camera center (the crosshair). Over the ocean it is
/// the region whose label is nearest within [`SNAP_RADIUS_PX`] screen pixels
/// (the short way around the world), or `None`.
fn crosshair_region(map: &WorldMap, camera: &Camera) -> Option<RegionId> {
    let raster = &map.raster;
    let x = camera.cx.floor() as i32;
    let y = (camera.cy.floor() as i32).min(i32::from(raster.height) - 1);
    if let Some(r) = raster.region_at(x, y) {
        return Some(r);
    }
    let radius = SNAP_RADIUS_PX * camera.pixel_scale().max(1.0);
    let w = camera.world_w.max(1.0);
    let mut best: Option<(f32, RegionId)> = None;
    for info in &map.regions {
        let mut dx = (f32::from(info.label.0) + 0.5 - camera.cx).abs() % w;
        if dx > w / 2.0 {
            dx = w - dx;
        }
        let dy = f32::from(info.label.1) + 0.5 - camera.cy;
        let d = (dx * dx + dy * dy).sqrt();
        let closer = match best {
            Some((bd, _)) => d < bd,
            None => true,
        };
        if d <= radius && closer {
            best = Some((d, info.id));
        }
    }
    best.map(|(_, r)| r)
}

/// Squared distance between the labels of `a` and `b` in raster cells, the
/// short way around the world.
fn label_dist2(map: &WorldMap, a: RegionId, b: RegionId) -> i64 {
    let (pa, pb) = match (map.get_region(a), map.get_region(b)) {
        (Some(x), Some(y)) => (x.label, y.label),
        _ => return i64::MAX,
    };
    let w = i64::from(map.raster.width.max(1));
    let mut dx = (i64::from(pa.0) - i64::from(pb.0)).abs() % w;
    if dx > w / 2 {
        dx = w - dx;
    }
    let dy = i64::from(pa.1) - i64::from(pb.1);
    dx * dx + dy * dy
}

/// The next (or previous) entry of a cycle through `list` after the one in
/// `memo`, which is updated. When the remembered region left the list, the
/// cycle goes on from the same position.
fn cycle_step(
    list: &[RegionId],
    memo: &mut Option<(RegionId, usize)>,
    forward: bool,
) -> Option<RegionId> {
    let n = list.len();
    if n == 0 {
        return None;
    }
    let idx = match *memo {
        None => {
            if forward {
                0
            } else {
                n - 1
            }
        }
        Some((last, i)) => match list.iter().position(|&r| r == last) {
            Some(p) if forward => (p + 1) % n,
            Some(p) => (p + n - 1) % n,
            None if forward => i % n,
            None => (i + n - 1) % n,
        },
    };
    *memo = Some((list[idx], idx));
    Some(list[idx])
}

/// Whether an action is allowed in a read-only session (replay or spectating).
fn allowed_read_only(a: &Action) -> bool {
    matches!(
        a,
        Action::Speed(_)
            | Action::TogglePause
            | Action::Focus(_)
            | Action::Info(_)
            | Action::Status
            | Action::Help(_)
            | Action::Quit
            | Action::Theme(_)
            | Action::ListAliases
    )
}

/// The region a parsed action points at (for the map highlight).
fn action_region(a: &Action) -> Option<RegionId> {
    match a {
        Action::Game(cmd) => cmd.region(),
        Action::Info(r) | Action::Focus(r) => Some(*r),
        _ => None,
    }
}

/// Runs the UI until the player quits. The terminal is restored on every exit
/// path (including errors and panics).
pub fn run_app(opts: UiOptions, persistence: Box<dyn Persistence>) -> io::Result<()> {
    let skip_splash = opts.skip_splash;
    let mut app = App::new(opts, persistence);
    let (guard, mut terminal) = TerminalGuard::enter()?;
    let (input, rx) = InputThread::start();
    if skip_splash {
        app.after_splash();
    }
    let result = app.main_loop(&mut terminal, &rx);
    drop(input);
    drop(terminal);
    drop(guard);
    result
}

impl App {
    fn new(opts: UiOptions, persistence: Box<dyn Persistence>) -> App {
        let UiOptions {
            settings,
            start,
            caps,
            map,
            balance,
            ..
        } = opts;
        let form_cfg: MatchConfig = match &start {
            StartMode::Setup(cfg) | StartMode::Immediate(cfg) => cfg.clone(),
            _ => settings.last_config.clone().unwrap_or_default(),
        };
        let form = SetupForm::from_config(&form_cfg, settings.theme, settings.renderer);
        let theme = Theme::get(settings.theme);
        let effective = caps.with_choice(settings.renderer);
        let history = History::new(settings.history_size.max(10));
        let send_pct = initial_send_pct(&settings);
        let camera = Camera::world(&map);
        let renderer = MapRenderer::new(Arc::clone(&map));
        let now = Instant::now();
        App {
            persistence,
            settings,
            base_caps: caps,
            caps: effective,
            theme,
            map,
            balance,
            renderer,
            camera,
            screen: Screen::Splash,
            screen_since: now,
            pending_start: Some(start),
            form,
            setup_msg: None,
            saves: Vec::new(),
            session: None,
            choose: None,
            prompt: Prompt::new(),
            prompt_open: false,
            history,
            analysis: Analysis::default(),
            prompt_target: None,
            submit_error: None,
            log: EventLog::new(),
            panel: None,
            help: None,
            speed_pct: 100,
            paused: false,
            send_pct,
            acc_us: 0,
            last_update: now,
            anim_frame: 0,
            last_anim: now,
            anim_active: false,
            tab_cycle: None,
            own_cycle: None,
            last_action: None,
            focus_flash: None,
            dirty: true,
            quit: false,
        }
    }

    // ------------------------------------------------------------------
    // Main loop
    // ------------------------------------------------------------------

    fn main_loop(
        &mut self,
        terminal: &mut DefaultTerminal,
        rx: &Receiver<io::Result<Event>>,
    ) -> io::Result<()> {
        let mut last_draw: Option<Instant> = None;
        loop {
            let now = Instant::now();
            self.update(now);
            if self.quit {
                return Ok(());
            }
            if self.dirty {
                let can_draw = match last_draw {
                    None => true,
                    Some(t) => now.saturating_duration_since(t) >= FRAME_MIN,
                };
                if can_draw {
                    terminal.draw(|f| view::draw(self, f))?;
                    self.dirty = false;
                    last_draw = Some(Instant::now());
                }
            }
            let timeout = self.next_timeout(Instant::now(), last_draw);
            match rx.recv_timeout(timeout) {
                Ok(ev) => {
                    self.handle_event(ev?);
                    while !self.quit {
                        match rx.try_recv() {
                            Ok(more) => self.handle_event(more?),
                            Err(_) => break,
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("the terminal input thread stopped"));
                }
            }
            if self.quit {
                return Ok(());
            }
        }
    }

    fn next_timeout(&self, now: Instant, last_draw: Option<Instant>) -> Duration {
        let mut t = IDLE_WAIT;
        if self.dirty {
            let wait = match last_draw {
                Some(ld) => FRAME_MIN.saturating_sub(now.saturating_duration_since(ld)),
                None => Duration::from_millis(0),
            };
            t = t.min(wait);
        }
        if self.anim_active {
            t = t.min(ANIM_STEP.saturating_sub(now.saturating_duration_since(self.last_anim)));
        }
        match self.screen {
            Screen::Playing => {
                if !self.paused && self.session.is_some() {
                    let left = TICK_US.saturating_sub(self.acc_us);
                    let speed = u64::from(self.speed_pct.max(1));
                    let us = left.saturating_mul(100) / speed;
                    t = t.min(Duration::from_micros(us));
                }
            }
            Screen::Splash | Screen::Countdown => t = t.min(ANIM_STEP),
            _ => {}
        }
        t.max(Duration::from_millis(1))
    }

    /// Whether something on screen is animating at `now`.
    fn animating(&self, now: Instant) -> bool {
        match self.screen {
            Screen::Splash | Screen::Countdown => true,
            Screen::ChooseStart => match &self.choose {
                Some(c) => c.resolved.is_some(),
                None => false,
            },
            Screen::Playing => {
                self.prompt_target.is_some()
                    || (!self.prompt_open && self.selected_region().is_some())
                    || self.focus_active(now)
                    || self.log.any_flashing(now)
            }
            _ => false,
        }
    }

    /// Whether the focus pulse is still running.
    pub fn focus_active(&self, now: Instant) -> bool {
        match self.focus_flash {
            Some((_, until)) => now < until,
            None => false,
        }
    }

    fn update(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.last_update);
        self.last_update = now;

        let anim = self.animating(now);
        if anim != self.anim_active {
            self.anim_active = anim;
            self.dirty = true;
        }
        let since_anim = now.saturating_duration_since(self.last_anim);
        if since_anim >= ANIM_STEP {
            if anim {
                let steps = (since_anim.as_millis() / ANIM_STEP.as_millis()).max(1);
                let steps = steps.min(1000) as u32;
                self.anim_frame = self.anim_frame.wrapping_add(steps);
                self.dirty = true;
            }
            self.last_anim = now;
        }

        let on_screen = now.saturating_duration_since(self.screen_since);
        match self.screen {
            Screen::Splash => {
                if on_screen >= SPLASH_TOTAL {
                    self.after_splash();
                }
            }
            Screen::Countdown => {
                if on_screen >= COUNT_STEP * 4 {
                    self.set_screen(Screen::Playing);
                    self.acc_us = 0;
                }
            }
            Screen::Playing => self.run_sim(dt),
            _ => {}
        }
    }

    fn run_sim(&mut self, dt: Duration) {
        if self.paused || self.session.is_none() {
            self.acc_us = 0;
            return;
        }
        let dt_us = dt.as_micros().min(1_000_000) as u64;
        self.acc_us = self
            .acc_us
            .saturating_add(dt_us.saturating_mul(u64::from(self.speed_pct)) / 100);
        let mut steps: u32 = 0;
        while self.acc_us >= TICK_US {
            self.acc_us -= TICK_US;
            self.step_once();
            steps += 1;
            if self.screen != Screen::Playing || self.paused {
                self.acc_us = 0;
                break;
            }
            if steps >= MAX_STEPS_PER_UPDATE {
                self.acc_us = 0;
                break;
            }
        }
        if steps > 0 {
            self.dirty = true;
            if !self.prompt.is_empty() {
                self.refresh_analysis();
            }
        }
    }

    fn step_once(&mut self) {
        let now = Instant::now();
        let already_done = match &self.session {
            Some(s) => s.replay_finished(),
            None => return,
        };
        if already_done {
            self.enter_game_over();
            return;
        }
        let events = match self.session.as_mut() {
            Some(s) => s.step(),
            None => return,
        };
        let mut human_died = false;
        if let Some(game) = session_game(&self.session) {
            for ev in &events {
                if let Some((msg, important, flash)) = log::describe_event(game, ev, HUMAN) {
                    self.log.push(ev.tick, msg, important, flash, now);
                }
                if let EventKind::Eliminated { player, .. } = &ev.kind {
                    if *player == HUMAN {
                        human_died = true;
                    }
                }
            }
        }
        let (over, is_replay, replay_done, watching) = match &self.session {
            Some(s) => (
                s.game().is_over(),
                s.is_replay(),
                s.replay_finished(),
                s.spectating || s.human_dead,
            ),
            None => return,
        };
        if over || replay_done {
            self.enter_game_over();
        } else if human_died && !is_replay && !watching {
            if let Some(s) = self.session.as_mut() {
                s.human_dead = true;
            }
            self.enter_game_over();
        }
    }

    fn set_screen(&mut self, s: Screen) {
        self.screen = s;
        self.screen_since = Instant::now();
        self.dirty = true;
    }

    fn current_tick(&self) -> u64 {
        match session_game(&self.session) {
            Some(g) => g.tick(),
            None => 0,
        }
    }

    /// Adds a one-tone message to the event log.
    fn note(&mut self, text: &str, tone: Tone) {
        let tick = self.current_tick();
        self.log.message(tick, text, tone, Instant::now());
        self.dirty = true;
    }

    fn persist_settings(&mut self) -> Option<String> {
        match self.persistence.save_settings(&self.settings) {
            Ok(()) => None,
            Err(e) => Some(format!("could not save settings: {}", e)),
        }
    }

    fn apply_theme(&mut self, kind: ThemeKind) {
        self.settings.theme = kind;
        self.form.theme = kind;
        self.theme = Theme::get(kind);
        self.renderer.invalidate();
        self.dirty = true;
    }

    fn apply_renderer(&mut self) {
        self.settings.renderer = self.form.renderer;
        self.caps = self.base_caps.with_choice(self.settings.renderer);
        self.renderer.invalidate();
        self.dirty = true;
    }

    // ------------------------------------------------------------------
    // Screen transitions
    // ------------------------------------------------------------------

    fn after_splash(&mut self) {
        let start = self.pending_start.take();
        match start {
            None | Some(StartMode::Setup(_)) => self.go_setup(None),
            Some(StartMode::Immediate(cfg)) => self.start_from_config(cfg),
            Some(StartMode::Resume(game)) => self.resume(*game, None),
            Some(StartMode::Replay(replay)) => self.start_replay(*replay),
        }
    }

    fn go_setup(&mut self, msg: Option<(String, bool)>) {
        self.session = None;
        self.choose = None;
        self.help = None;
        self.panel = None;
        self.prompt.clear();
        self.prompt_open = false;
        self.analysis = Analysis::default();
        self.prompt_target = None;
        self.submit_error = None;
        self.camera = Camera::world(&self.map);
        self.saves = self.persistence.list_saves();
        self.setup_msg = msg;
        self.set_screen(Screen::Setup);
    }

    fn start_from_config(&mut self, cfg: MatchConfig) {
        let mut cfg = cfg;
        if let Err(e) = cfg.validate(&self.map) {
            self.go_setup(Some((format!("Invalid match settings: {}", e), true)));
            return;
        }
        if cfg.seed == DEFAULT_SEED {
            cfg.seed = crate::setup::random_seed();
        }
        self.start_match(cfg);
    }

    fn start_match(&mut self, cfg: MatchConfig) {
        let spectate = cfg.spectate;
        let game = Game::new(Arc::clone(&self.map), self.balance.clone(), cfg);
        self.begin_session(Driver::Live(Box::new(game)), true);
        let summary = match session_game(&self.session) {
            Some(g) => {
                let c = g.config();
                let timer = match c.active_time_limit_secs() {
                    Some(s) => log::clock(u64::from(s)),
                    None => "no timer".to_string(),
                };
                format!(
                    "Match started: {}, {} bots ({}), {}, {}, seed {}",
                    c.preset.name(),
                    c.bots,
                    c.difficulty.name(),
                    c.win.label(),
                    timer,
                    c.seed
                )
            }
            None => String::new(),
        };
        self.note(&summary, Tone::Accent);
        if spectate {
            self.note(
                "Bot-only match: you are watching. Arrows pan, p pauses, [ ] change speed, i shows info, :q quits.",
                Tone::Accent,
            );
        } else {
            self.note(
                "Arrows move the crosshair, Tab jumps to a target, a attacks it with the amount set by 1-0. : types a command, ? helps.",
                Tone::Dim,
            );
        }
    }

    fn resume(&mut self, game: Game, name: Option<&str>) {
        let mut game = game;
        if let Err(e) = game.check_map(&self.map) {
            let what = match name {
                Some(n) => format!("Could not load '{}': {}", n, e),
                None => format!("Could not resume the match: {}", e),
            };
            self.go_setup(Some((what, true)));
            return;
        }
        game.attach_map(Arc::clone(&self.map));
        let when = log::clock(game.elapsed_secs());
        self.begin_session(Driver::Live(Box::new(game)), true);
        let text = match name {
            Some(n) => format!("Resumed save '{}' at {}", n, when),
            None => format!("Resumed match at {}", when),
        };
        self.note(&text, Tone::Accent);
    }

    fn start_replay(&mut self, replay: Replay) {
        let commands = replay.commands.len();
        let final_tick = replay.final_tick;
        let seed = replay.config.seed;
        let player = match ReplayPlayer::try_new(Arc::clone(&self.map), replay) {
            Ok(p) => p,
            Err(e) => {
                self.go_setup(Some((format!("Could not play the replay: {}", e), true)));
                return;
            }
        };
        self.begin_session(Driver::Replay(Box::new(player)), false);
        self.note(
            &format!(
                "Replay: seed {}, {} commands, {} long. Read-only: speed, pause, focus, info, quit.",
                seed,
                commands,
                log::clock(final_tick / TICKS_PER_SECOND)
            ),
            Tone::Accent,
        );
    }

    fn begin_session(&mut self, driver: Driver, countdown: bool) {
        let mut session = Session::new(driver);
        let (live, over, spectate, human_alive) = {
            let g = session.game();
            (
                !session.is_replay(),
                g.is_over(),
                g.config().spectate,
                g.is_alive(HUMAN),
            )
        };
        if live && !over {
            if spectate {
                // A bot-only match (`--bot-only-watch`): player 0 is a bot too,
                // so the viewer only watches.
                session.spectating = true;
            } else if !human_alive {
                session.human_dead = true;
                session.spectating = true;
            }
        }
        let capital = match session.game().get_player(HUMAN) {
            Some(p) => p.capital,
            None => None,
        };
        self.session = Some(session);
        self.choose = None;
        self.log.clear();
        self.prompt.clear();
        self.prompt_open = false;
        self.history.reset_navigation();
        self.analysis = Analysis::default();
        self.prompt_target = None;
        self.submit_error = None;
        self.panel = None;
        self.help = None;
        self.speed_pct = 100;
        self.paused = false;
        self.send_pct = initial_send_pct(&self.settings);
        self.acc_us = 0;
        self.tab_cycle = None;
        self.own_cycle = None;
        self.last_action = None;
        self.focus_flash = None;
        self.camera = Camera::world(&self.map);
        if let Some(r) = capital {
            self.camera.focus_region(&self.map, r);
        }
        self.renderer.invalidate();
        if countdown {
            self.set_screen(Screen::Countdown);
        } else {
            self.set_screen(Screen::Playing);
        }
    }

    fn enter_game_over(&mut self) {
        let mut saved: Option<Result<String, String>> = None;
        if let Some(s) = self.session.as_mut() {
            let replay = match &s.driver {
                Driver::Live(g) if !s.replay_saved => Some(g.to_replay()),
                _ => None,
            };
            if let Some(replay) = replay {
                let res = self.persistence.save_replay(&replay);
                s.replay_saved = true;
                s.replay_path = Some(res.clone());
                saved = Some(res);
            }
        }
        match saved {
            Some(Ok(path)) => self.note(&format!("Replay saved to {}", path), Tone::Dim),
            Some(Err(e)) => self.note(&format!("Could not save the replay: {}", e), Tone::Bad),
            None => {}
        }
        self.help = None;
        self.paused = false;
        self.acc_us = 0;
        self.prompt.clear();
        self.prompt_open = false;
        self.analysis = Analysis::default();
        self.prompt_target = None;
        self.submit_error = None;
        self.set_screen(Screen::GameOver);
    }

    fn enter_choose(&mut self, cfg: MatchConfig) {
        let mut pcfg = cfg.clone();
        pcfg.bots = 0;
        pcfg.start = StartChoice::Random;
        let preview = Game::new(Arc::clone(&self.map), self.balance.clone(), pcfg);
        self.camera = Camera::world(&self.map);
        self.prompt.clear();
        self.prompt_open = false;
        self.tab_cycle = None;
        self.choose = Some(ChooseState {
            config: cfg,
            preview,
            resolved: None,
            error: None,
        });
        self.set_screen(Screen::ChooseStart);
        self.choose_track();
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Release {
                    self.handle_key(key);
                }
            }
            Event::Resize(_, _) => {
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Windows reports AltGr as Ctrl+Alt, so Ctrl+Alt+C is a typed character.
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if ctrl && !alt {
            if let KeyCode::Char(c) = key.code {
                if c == 'c' || c == 'C' {
                    self.quit = true;
                    return;
                }
            }
        }
        self.dirty = true;
        // While the crosshair shows (hotkey mode), the center may go past the
        // band the view can center on, so the crosshair reaches every row.
        self.camera.free_y = !self.prompt_open;
        match self.screen {
            Screen::Splash => self.after_splash(),
            Screen::Setup => self.key_setup(key),
            Screen::ChooseStart => self.key_choose(key),
            Screen::Countdown => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ')) {
                    self.set_screen(Screen::Playing);
                    self.acc_us = 0;
                }
            }
            Screen::Playing => self.key_playing(key),
            Screen::GameOver => self.key_game_over(key),
        }
    }

    fn key_setup(&mut self, key: KeyEvent) {
        if let Some(overlay) = self.help.as_mut() {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.help = None,
                KeyCode::Up => overlay.scroll = overlay.scroll.saturating_sub(1),
                KeyCode::Down => overlay.scroll = (overlay.scroll + 1).min(overlay.lines.len().saturating_sub(1)),
                KeyCode::PageUp => overlay.scroll = overlay.scroll.saturating_sub(8),
                KeyCode::PageDown => overlay.scroll = (overlay.scroll + 8).min(overlay.lines.len().saturating_sub(1)),
                KeyCode::Home => overlay.scroll = 0,
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::BackTab => self.form.up(),
            KeyCode::Down | KeyCode::Tab => self.form.down(),
            KeyCode::Left => self.form_change(-1),
            KeyCode::Right | KeyCode::Char(' ') => self.form_change(1),
            KeyCode::Backspace => self.form.backspace(),
            KeyCode::Enter => match self.form.field() {
                Field::Play => self.setup_start(),
                Field::HowToPlay => self.open_setup_help(),
                _ => self.form_change(1),
            },
            KeyCode::Esc => self.quit = true,
            KeyCode::Char(c) => {
                if let Some(d) = c.to_digit(10) {
                    if !self.form.digit(d) {
                        self.setup_msg = Some((
                            "Digits only go into numeric fields (length, bots, domination %, seed)."
                                .to_string(),
                            false,
                        ));
                    }
                } else if c == 'q' || c == 'Q' {
                    self.quit = true;
                } else if c == 'h' || c == 'H' {
                    self.open_setup_help();
                } else if c == 'l' || c == 'L' {
                    self.load_latest();
                }
            }
            _ => {}
        }
    }

    fn open_setup_help(&mut self) {
        self.help = Some(HelpOverlay { lines: help::before_play(), scroll: 0 });
    }

    fn form_change(&mut self, dir: i32) {
        match self.form.change(dir) {
            FormChange::None => {}
            FormChange::Theme => {
                let kind = self.form.theme;
                self.apply_theme(kind);
                if let Some(e) = self.persist_settings() {
                    self.setup_msg = Some((e, true));
                }
            }
            FormChange::Renderer => {
                self.apply_renderer();
                if let Some(e) = self.persist_settings() {
                    self.setup_msg = Some((e, true));
                }
            }
        }
    }

    fn setup_start(&mut self) {
        let (cfg, remembered) = self.form.build();
        if let Err(e) = cfg.validate(&self.map) {
            self.setup_msg = Some((e, true));
            return;
        }
        self.settings.last_config = Some(remembered);
        if let Some(e) = self.persist_settings() {
            self.setup_msg = Some((e, true));
        }
        if self.form.choose_start {
            self.enter_choose(cfg);
        } else {
            self.start_match(cfg);
        }
    }

    fn load_latest(&mut self) {
        self.saves = self.persistence.list_saves();
        let name = match self.saves.first() {
            Some(n) => n.clone(),
            None => {
                self.setup_msg = Some(("There are no saved matches yet.".to_string(), false));
                return;
            }
        };
        match self.persistence.load_game(&name) {
            Ok(game) => self.resume(game, Some(name.as_str())),
            Err(e) => {
                self.setup_msg = Some((format!("Could not load '{}': {}", name, e), true));
            }
        }
    }

    fn key_choose(&mut self, key: KeyEvent) {
        if self.prompt_open {
            self.key_choose_prompt(key);
        } else {
            self.key_choose_hotkey(key);
        }
    }

    /// The start picker in hotkey mode: move the crosshair onto a region and
    /// press Enter to start there.
    fn key_choose_hotkey(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => {
                self.go_setup(None);
                return;
            }
            KeyCode::Enter => {
                self.choose_pick();
                return;
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.pan_key(key.code, ctrl || shift);
            }
            KeyCode::Tab => self.choose_cycle(!shift),
            KeyCode::BackTab => self.choose_cycle(false),
            KeyCode::Char(c) => {
                // Ctrl or Alt combinations are not hotkeys, but Windows reports
                // AltGr (needed for ':' on some layouts) as Ctrl+Alt.
                if ctrl != alt {
                    return;
                }
                match c {
                    ':' => {
                        self.prompt.clear();
                        self.prompt_open = true;
                        if let Some(ch) = self.choose.as_mut() {
                            ch.error = None;
                        }
                        return;
                    }
                    '+' | '=' | '-' | '_' => self.zoom_key(c),
                    _ => return,
                }
            }
            _ => return,
        }
        self.choose_track();
    }

    /// The start picker's typed prompt (opened with `:`): type a region name.
    fn key_choose_prompt(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let mut edited = false;
        match key.code {
            KeyCode::Esc => {
                if self.prompt.is_empty() {
                    self.prompt_open = false;
                    self.choose_track();
                    return;
                }
                self.prompt.clear();
                edited = true;
            }
            KeyCode::Enter => {
                self.choose_confirm();
                return;
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if ctrl || shift => {
                self.pan_key(key.code, false);
            }
            KeyCode::Left => {
                if self.prompt.is_empty() {
                    self.pan_key(key.code, false);
                } else {
                    self.prompt.left();
                }
            }
            KeyCode::Right => {
                if self.prompt.is_empty() {
                    self.pan_key(key.code, false);
                } else {
                    self.prompt.right();
                }
            }
            KeyCode::Home => self.prompt.home(),
            KeyCode::End => self.prompt.end(),
            KeyCode::Backspace => edited = self.prompt.backspace(),
            KeyCode::Delete => edited = self.prompt.delete(),
            KeyCode::Char(c) => {
                // Windows reports AltGr (needed for '@' on many layouts) as Ctrl+Alt.
                let altgr = ctrl && alt;
                if ctrl && !altgr {
                    if c == 'u' {
                        self.prompt.clear();
                        edited = true;
                    } else if c == 'w' {
                        edited = self.prompt.delete_word();
                    }
                } else if !alt || altgr {
                    if (c == '+' || c == '-') && self.prompt.is_empty() {
                        self.zoom_key(c);
                    } else {
                        self.prompt.insert(c);
                        edited = true;
                    }
                }
            }
            _ => {}
        }
        if edited {
            self.choose_resolve();
        }
    }

    fn choose_resolve(&mut self) {
        let text = self.prompt.text().trim().to_string();
        let map = Arc::clone(&self.map);
        let balance = self.balance.clone();
        let mut focus: Option<RegionId> = None;
        if let Some(c) = self.choose.as_mut() {
            if text.is_empty() {
                c.resolved = None;
                c.error = None;
            } else {
                match fuzzy::resolve_region(&c.preview, HUMAN, &text, Relevance::Any) {
                    Ok(r) => {
                        if c.resolved != Some(r) {
                            let mut pcfg = c.config.clone();
                            pcfg.bots = 0;
                            pcfg.start = StartChoice::Region(r);
                            c.preview = Game::new(Arc::clone(&map), balance, pcfg);
                            focus = Some(r);
                        }
                        c.resolved = Some(r);
                        c.error = None;
                    }
                    Err(e) => {
                        c.resolved = None;
                        c.error = Some(e);
                    }
                }
            }
        }
        if let Some(r) = focus {
            self.camera.focus_region(&self.map, r);
        }
    }

    /// Hotkey mode on the start picker: the region under the crosshair is the
    /// pick, and the preview shows you starting there.
    fn choose_track(&mut self) {
        let sel = self.selected_region();
        let map = Arc::clone(&self.map);
        let balance = self.balance.clone();
        if let Some(c) = self.choose.as_mut() {
            c.error = None;
            if c.resolved == sel {
                return;
            }
            c.resolved = sel;
            if let Some(r) = sel {
                let mut pcfg = c.config.clone();
                pcfg.bots = 0;
                pcfg.start = StartChoice::Region(r);
                c.preview = Game::new(map, balance, pcfg);
            }
        }
    }

    /// Tab on the start picker: jump through all regions, most valuable first.
    fn choose_cycle(&mut self, forward: bool) {
        let map = Arc::clone(&self.map);
        let mut list: Vec<RegionId> = map.regions.iter().map(|info| info.id).collect();
        list.sort_by_key(|&r| (Reverse(map.region(r).value), r));
        if let Some(r) = cycle_step(&list, &mut self.tab_cycle, forward) {
            self.snap_to(r);
        }
    }

    /// Enter on the start picker in hotkey mode: start in the region under the crosshair.
    fn choose_pick(&mut self) {
        self.choose_track();
        let pick: Option<(MatchConfig, RegionId)> = match &self.choose {
            Some(c) => c.resolved.map(|r| (c.config.clone(), r)),
            None => None,
        };
        match pick {
            Some((cfg, r)) => {
                let mut cfg = cfg;
                cfg.start = StartChoice::Region(r);
                self.start_match(cfg);
            }
            None => {
                if let Some(c) = self.choose.as_mut() {
                    c.error = Some(
                        "move the crosshair onto a region first (arrows or Tab), or press : to type one"
                            .to_string(),
                    );
                }
            }
        }
    }

    fn choose_confirm(&mut self) {
        let empty = self.prompt.text().trim().is_empty();
        let pick: Option<(MatchConfig, RegionId)> = match &self.choose {
            Some(c) => {
                let region = if empty {
                    match c.preview.get_player(HUMAN) {
                        Some(p) => p.capital,
                        None => None,
                    }
                } else {
                    c.resolved
                };
                region.map(|r| (c.config.clone(), r))
            }
            None => None,
        };
        match pick {
            Some((cfg, r)) => {
                let mut cfg = cfg;
                cfg.start = StartChoice::Region(r);
                self.start_match(cfg);
            }
            None => {
                if let Some(c) = self.choose.as_mut() {
                    if c.error.is_none() {
                        c.error = Some("type a region name or code".to_string());
                    }
                }
            }
        }
    }

    fn key_game_over(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.go_setup(None),
            KeyCode::Char('q') | KeyCode::Char('Q') => self.quit = true,
            KeyCode::Char('w') | KeyCode::Char('W') => {
                let can_watch = match &self.session {
                    Some(s) => {
                        s.human_dead && !s.abandoned && !s.is_replay() && !s.game().is_over()
                    }
                    None => false,
                };
                if can_watch {
                    if let Some(s) = self.session.as_mut() {
                        s.spectating = true;
                    }
                    self.set_screen(Screen::Playing);
                    self.acc_us = 0;
                    self.note(
                        "Spectating: watch the bots finish. speed, pause, focus and quit still work.",
                        Tone::Accent,
                    );
                }
            }
            _ => {}
        }
    }

    /// Pans the camera (and so the crosshair) one step, or four with `fast`.
    fn pan_key(&mut self, code: KeyCode, fast: bool) {
        let k = if fast { 4 } else { 1 };
        let (sx, sy) = PAN_STEP;
        match code {
            KeyCode::Left => self.camera.pan(-sx * k, 0),
            KeyCode::Right => self.camera.pan(sx * k, 0),
            KeyCode::Up => self.camera.pan(0, -sy * k),
            KeyCode::Down => self.camera.pan(0, sy * k),
            _ => {}
        }
    }

    /// `+` or `=` zooms in, anything else (`-`, `_`) zooms out.
    fn zoom_key(&mut self, c: char) {
        if c == '+' || c == '=' {
            self.camera.zoom_in();
        } else {
            self.camera.zoom_out();
        }
    }

    /// PageUp / PageDown: scroll the help overlay when it is open, else the event log.
    fn scroll_page(&mut self, up: bool) {
        match self.help.as_mut() {
            Some(h) => {
                if up {
                    h.scroll = h.scroll.saturating_sub(8);
                } else {
                    h.scroll = (h.scroll + 8).min(h.lines.len().saturating_sub(1));
                }
            }
            None => {
                if up {
                    self.log.scroll_up(5);
                } else {
                    self.log.scroll_down(5);
                }
            }
        }
    }

    fn key_playing(&mut self, key: KeyEvent) {
        if self.prompt_open {
            self.key_prompt(key);
        } else {
            self.key_hotkey(key);
        }
    }

    /// Opens prompt mode (the typed command line).
    fn open_prompt(&mut self) {
        self.prompt_open = true;
        self.submit_error = None;
        self.history.reset_navigation();
    }

    /// Leaves prompt mode for hotkey mode, dropping the line.
    fn close_prompt(&mut self) {
        self.prompt_open = false;
        self.prompt.clear();
        self.history.reset_navigation();
        self.analysis = Analysis::default();
        self.prompt_target = None;
        self.submit_error = None;
    }

    /// Hotkey mode (the default while playing): single keys act at once, on
    /// the region under the crosshair where they need one. Unknown keys do nothing.
    fn key_hotkey(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        if let Some(h) = self.help.as_mut() {
            // The help overlay takes the keys that scroll or close it.
            match key.code {
                KeyCode::Up => {
                    h.scroll = h.scroll.saturating_sub(1);
                    return;
                }
                KeyCode::Down => {
                    h.scroll = (h.scroll + 1).min(h.lines.len().saturating_sub(1));
                    return;
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('h') => {
                    self.help = None;
                    return;
                }
                KeyCode::PageUp | KeyCode::PageDown => {}
                // Swallow everything else so a stray letter can't fire an
                // action on the map hidden under the overlay.
                _ => return,
            }
        }
        if key.code == KeyCode::Esc {
            if self.submit_error.is_some() {
                self.submit_error = None;
            } else {
                self.panel = None;
            }
            return;
        }
        // Each key replaces the previous key's error.
        self.submit_error = None;
        match key.code {
            KeyCode::Enter => self.open_prompt(),
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.pan_key(key.code, ctrl || shift);
            }
            KeyCode::Tab => self.cycle_targets(!shift),
            KeyCode::BackTab => self.cycle_targets(false),
            KeyCode::PageUp => self.scroll_page(true),
            KeyCode::PageDown => self.scroll_page(false),
            KeyCode::Char(c) => {
                // Ctrl or Alt combinations are not hotkeys, but Windows reports
                // AltGr (needed for `[`, `]` or `:` on some layouts) as Ctrl+Alt.
                if ctrl == alt {
                    self.hotkey_char(c);
                }
            }
            _ => {}
        }
    }

    fn hotkey_char(&mut self, c: char) {
        let amount = Amount::Percent(self.send_pct);
        // Caps Lock shouldn't disable the hotkeys; `O` is the only uppercase binding.
        let c = if c == 'O' { c } else { c.to_ascii_lowercase() };
        match c {
            ':' => self.open_prompt(),
            '+' | '=' | '-' | '_' => self.zoom_key(c),
            'o' => self.cycle_owned(true),
            'O' => self.cycle_owned(false),
            'c' => self.snap_capital(),
            'a' => self.hotkey_on_selection("attack", |r| Command::Attack { target: r, amount }),
            'f' => self.hotkey_on_selection("fortify", |r| Command::Fortify { region: r, amount }),
            'r' => self.hotkey_on_selection("recall", |r| Command::Recall { region: r, amount }),
            'w' => self.hotkey_smart(SmartTarget::Weakest, "@weakest"),
            'e' => self.hotkey_smart(SmartTarget::Richest, "@richest"),
            '0'..='9' => {
                if let Some(d) = c.to_digit(10) {
                    self.send_pct = if d == 0 { 100 } else { (d * 10) as u8 };
                }
            }
            'i' => {
                if let Some(r) = self.require_selection() {
                    self.panel = Some(Panel::Region(r));
                }
            }
            's' => self.perform(Action::Status, ""),
            'l' => self.hotkey_diplomacy(true),
            'u' => self.hotkey_diplomacy(false),
            'y' => self.hotkey_accept_offer(),
            'p' | ' ' => self.perform(Action::TogglePause, ""),
            '[' => self.step_speed(false),
            ']' => self.step_speed(true),
            't' => self.perform(Action::Theme(None), ""),
            '?' | 'h' => self.perform(Action::Help(None), ""),
            '.' => self.repeat_last(),
            _ => {}
        }
    }

    /// The region under the map crosshair (the hotkey selection).
    pub fn selected_region(&self) -> Option<RegionId> {
        crosshair_region(&self.map, &self.camera)
    }

    /// The selection, or `None` with an error saying how to select a region.
    fn require_selection(&mut self) -> Option<RegionId> {
        let sel = self.selected_region();
        if sel.is_none() {
            self.submit_error = Some(
                "no region under the crosshair: move it with the arrows or press Tab".to_string(),
            );
        }
        sel
    }

    /// Whether the session only allows view actions (replay or spectating).
    fn read_only(&self) -> bool {
        match &self.session {
            Some(s) => s.read_only(),
            None => true,
        }
    }

    /// Shows the read-only error for `verb` and returns true in a read-only session.
    fn blocked(&mut self, verb: &str) -> bool {
        if self.read_only() {
            self.submit_error = Some(read_only_error(verb));
            return true;
        }
        false
    }

    /// Centers the crosshair on region `r`'s label, keeping the zoom. If the
    /// center is still kept off the label (the camera's band clamp, outside
    /// hotkey mode), it zooms in until the label is under the crosshair.
    fn snap_to(&mut self, r: RegionId) {
        let (x, y) = match self.map.get_region(r) {
            Some(info) => (f32::from(info.label.0) + 0.5, f32::from(info.label.1) + 0.5),
            None => return,
        };
        self.camera.center_on(x, y);
        while self.selected_region() != Some(r) && self.camera.zoom != ZoomLevel::Region {
            self.camera.zoom_in();
            self.camera.center_on(x, y);
        }
    }

    /// Tab / Shift+Tab: the next or previous region you can attack, nearest
    /// to your capital first.
    fn cycle_targets(&mut self, forward: bool) {
        let list: Vec<RegionId> = match session_game(&self.session) {
            Some(g) => {
                let mut list = g.attackable_regions(HUMAN);
                if let Some(cap) = g.get_player(HUMAN).and_then(|p| p.capital) {
                    let map = g.map();
                    list.sort_by_key(|&r| (label_dist2(map, cap, r), r));
                }
                list
            }
            None => return,
        };
        match cycle_step(&list, &mut self.tab_cycle, forward) {
            Some(r) => self.snap_to(r),
            None => {
                self.submit_error = Some("there is no region you can attack".to_string());
            }
        }
    }

    /// `o` / `O`: the next or previous region you own, most valuable first.
    fn cycle_owned(&mut self, forward: bool) {
        let list: Vec<RegionId> = match session_game(&self.session) {
            Some(g) => {
                let mut list = g.owned_regions(HUMAN);
                let map = g.map();
                list.sort_by_key(|&r| (Reverse(map.region(r).value), r));
                list
            }
            None => return,
        };
        match cycle_step(&list, &mut self.own_cycle, forward) {
            Some(r) => self.snap_to(r),
            None => self.submit_error = Some("you own no regions".to_string()),
        }
    }

    /// `c`: back to your capital.
    fn snap_capital(&mut self) {
        let capital = session_game(&self.session)
            .and_then(|g| g.get_player(HUMAN))
            .and_then(|p| p.capital);
        match capital {
            Some(r) => self.snap_to(r),
            None => self.submit_error = Some("you have no capital".to_string()),
        }
    }

    /// `[` / `]`: the next slower or faster step of [`SPEED_STEPS`].
    fn step_speed(&mut self, faster: bool) {
        let cur = self.speed_pct;
        let next = if faster {
            SPEED_STEPS.iter().copied().find(|&s| s > cur).unwrap_or(SPEED_MAX_PCT)
        } else {
            SPEED_STEPS.iter().rev().copied().find(|&s| s < cur).unwrap_or(SPEED_MIN_PCT)
        };
        self.perform(Action::Speed(next), "");
    }

    /// Runs the game command `build` makes for the selected region.
    fn hotkey_on_selection(&mut self, verb: &str, build: impl FnOnce(RegionId) -> Command) {
        if self.blocked(verb) {
            return;
        }
        if let Some(r) = self.require_selection() {
            self.hotkey_command(build(r), None);
        }
    }

    /// `w` / `e`: attack a smart target with the current amount and jump there.
    fn hotkey_smart(&mut self, target: SmartTarget, label: &str) {
        if self.blocked("attack") {
            return;
        }
        let resolved = match session_game(&self.session) {
            Some(g) => resolve_target(g, HUMAN, &target),
            None => return,
        };
        match resolved {
            Ok(r) => {
                self.snap_to(r);
                let amount = Amount::Percent(self.send_pct);
                self.hotkey_command(Command::Attack { target: r, amount }, Some(label));
            }
            Err(e) => self.submit_error = Some(format!("{}: {}", label, e)),
        }
    }

    /// `l` / `u`: ally with, or break with, the owner of the selected region.
    fn hotkey_diplomacy(&mut self, ally: bool) {
        if self.blocked(if ally { "ally" } else { "unally" }) {
            return;
        }
        let r = match self.require_selection() {
            Some(r) => r,
            None => return,
        };
        let owner = match session_game(&self.session) {
            Some(g) => g.region_state(r).owner,
            None => return,
        };
        let with = match owner {
            Some(p) => p,
            None => {
                let name = self.map.region(r).name.clone();
                self.submit_error = Some(format!(
                    "{} is neutral: select a region another player owns",
                    name
                ));
                return;
            }
        };
        let cmd = if ally {
            Command::ProposeAlliance { with }
        } else {
            Command::BreakAlliance { with }
        };
        self.hotkey_command(cmd, None);
    }

    /// `y`: accept the newest alliance proposal made to you.
    fn hotkey_accept_offer(&mut self) {
        if self.blocked("ally") {
            return;
        }
        let from = match session_game(&self.session) {
            Some(g) => g
                .proposals()
                .iter()
                .filter(|p| p.to == HUMAN)
                .max_by_key(|p| p.expires)
                .map(|p| p.from),
            None => return,
        };
        match from {
            Some(p) => self.hotkey_command(Command::ProposeAlliance { with: p }, None),
            None => self.submit_error = Some("no alliance offer to accept".to_string()),
        }
    }

    /// Runs a game command built by a hotkey through the same path as a typed
    /// one: `Game::preview` validates it (a rejection shows its reason and
    /// queues nothing), then it is queued and flashed in the event log.
    fn hotkey_command(&mut self, cmd: Command, smart: Option<&str>) {
        if self.blocked(cmd.verb()) {
            return;
        }
        let parsed = match session_game(&self.session) {
            Some(game) => {
                let ctx = make_ctx(game, &self.settings, &self.history);
                describe_command(cmd, smart, &ctx)
            }
            None => return,
        };
        match parsed {
            Ok(pc) => {
                self.submit_error = None;
                self.last_action = Some(Repeat::Command(cmd));
                self.perform(pc.action, &pc.hint);
            }
            Err(e) => self.submit_error = Some(e),
        }
    }

    /// `.`: repeats the last hotkey game command (same target) or command line.
    fn repeat_last(&mut self) {
        match self.last_action.clone() {
            Some(Repeat::Command(cmd)) => self.hotkey_command(cmd, None),
            Some(Repeat::Line(line)) => {
                if let Some(ran) = self.run_line(&line) {
                    self.last_action = Some(Repeat::Line(ran));
                }
            }
            None => self.submit_error = Some("nothing to repeat yet".to_string()),
        }
    }

    /// Prompt mode: the typed command line. Keys edit the line; Enter runs it
    /// and returns to hotkey mode; Esc clears the line, then returns.
    fn key_prompt(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let mut edited = false;
        match key.code {
            KeyCode::Esc => {
                if self.help.is_some() {
                    self.help = None;
                } else if !self.prompt.is_empty() {
                    self.prompt.clear();
                    edited = true;
                } else {
                    self.close_prompt();
                }
                self.history.reset_navigation();
            }
            KeyCode::Enter => {
                self.submit_line();
                return;
            }
            KeyCode::Tab => {
                if !self.analysis.ghost.is_empty() && self.prompt.at_end() {
                    let line = self.analysis.accept_ghost(self.prompt.text());
                    self.prompt.set(line);
                    edited = true;
                }
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if ctrl || shift => {
                self.pan_key(key.code, false);
            }
            KeyCode::Up => {
                if let Some(line) = self.history.up(self.prompt.text()) {
                    self.prompt.set(line);
                    self.after_edit();
                }
            }
            KeyCode::Down => {
                if let Some(line) = self.history.down() {
                    self.prompt.set(line);
                    self.after_edit();
                }
            }
            KeyCode::Left => {
                if self.prompt.is_empty() {
                    self.pan_key(key.code, false);
                } else {
                    self.prompt.left();
                }
            }
            KeyCode::Right => {
                if self.prompt.is_empty() {
                    self.pan_key(key.code, false);
                } else if self.prompt.at_end() && !self.analysis.ghost.is_empty() {
                    let line = self.analysis.accept_ghost(self.prompt.text());
                    self.prompt.set(line);
                    edited = true;
                } else {
                    self.prompt.right();
                }
            }
            KeyCode::Home => self.prompt.home(),
            KeyCode::End => self.prompt.end(),
            KeyCode::Backspace => edited = self.prompt.backspace(),
            KeyCode::Delete => edited = self.prompt.delete(),
            KeyCode::PageUp => self.scroll_page(true),
            KeyCode::PageDown => self.scroll_page(false),
            KeyCode::Char(c) => {
                // Windows reports AltGr (needed for '@' on many layouts) as Ctrl+Alt.
                let altgr = ctrl && alt;
                if ctrl && !altgr {
                    match c {
                        'u' => {
                            self.prompt.clear();
                            edited = true;
                        }
                        'w' => edited = self.prompt.delete_word(),
                        'a' => self.prompt.home(),
                        'e' => self.prompt.end(),
                        _ => {}
                    }
                } else if !alt || altgr {
                    if (c == '+' || c == '-') && self.prompt.is_empty() {
                        self.zoom_key(c);
                    } else {
                        self.prompt.insert(c);
                        edited = true;
                    }
                }
            }
            _ => {}
        }
        if edited {
            self.history.reset_navigation();
            self.after_edit();
        }
    }

    fn after_edit(&mut self) {
        self.submit_error = None;
        self.refresh_analysis();
    }

    /// Re-analyzes the prompt (hint strip, ghost text, highlighted target).
    fn refresh_analysis(&mut self) {
        if self.prompt.is_empty() {
            self.analysis = Analysis::default();
            self.prompt_target = None;
            return;
        }
        let analysis = match session_game(&self.session) {
            Some(game) => {
                let ctx = make_ctx(game, &self.settings, &self.history);
                command::analyze(self.prompt.text(), &ctx)
            }
            None => Analysis::default(),
        };
        let mut target: Option<RegionId> = None;
        if let Some(seg) = analysis.segments.last() {
            if let Ok(pc) = &seg.result {
                target = action_region(&pc.action);
            }
        }
        self.analysis = analysis;
        self.prompt_target = target;
        self.dirty = true;
    }

    /// Enter in prompt mode: runs the line and returns to hotkey mode; a
    /// rejected line stays on the prompt with its reason. An empty line just
    /// returns to hotkey mode.
    fn submit_line(&mut self) {
        let line = self.prompt.text().to_string();
        if line.trim().is_empty() {
            self.close_prompt();
            return;
        }
        if let Some(ran) = self.run_line(&line) {
            self.last_action = Some(Repeat::Line(ran));
            self.close_prompt();
        }
    }

    /// Parses, checks and performs a whole command line exactly as typed at
    /// the prompt. Returns the line `.` should repeat (with `!!` expanded), or
    /// `None` when the line was rejected (the reason is in `submit_error`).
    fn run_line(&mut self, line: &str) -> Option<String> {
        let result: SubmitResult = match session_game(&self.session) {
            Some(game) => {
                let ctx = make_ctx(game, &self.settings, &self.history);
                command::submit(line, &ctx)
            }
            None => return None,
        };
        let SubmitResult {
            actions,
            echoes,
            errors,
            history_line,
        } = result;
        if !errors.is_empty() {
            self.submit_error = Some(errors.join(" | "));
            self.dirty = true;
            return None;
        }
        if self.read_only() {
            if let Some(bad) = actions.iter().find(|a| !allowed_read_only(a)) {
                let what = match bad {
                    Action::Game(cmd) => cmd.verb().to_string(),
                    Action::Save(_) => "save".to_string(),
                    _ => "that command".to_string(),
                };
                self.submit_error = Some(read_only_error(&what));
                return None;
            }
        }
        let ran = match &history_line {
            Some(h) => h.clone(),
            None => line.to_string(),
        };
        if let Some(h) = history_line {
            self.history.push(&h);
        } else {
            self.history.reset_navigation();
        }
        self.submit_error = None;
        for (i, action) in actions.into_iter().enumerate() {
            let echo = match echoes.get(i) {
                Some(e) => e.clone(),
                None => String::new(),
            };
            self.perform(action, &echo);
        }
        Some(ran)
    }

    fn echo(&mut self, echo: &str, flash: bool) {
        if echo.is_empty() {
            return;
        }
        let tick = self.current_tick();
        let msg = Msg::new().push("> ", Tone::Dim).push(echo, Tone::Accent);
        self.log.push(tick, msg, true, flash, Instant::now());
        self.dirty = true;
    }

    fn perform(&mut self, action: Action, echo: &str) {
        match action {
            Action::Game(cmd) => {
                let mut queued = false;
                if let Some(s) = self.session.as_mut() {
                    if let Driver::Live(g) = &mut s.driver {
                        g.queue(HUMAN, cmd);
                        queued = true;
                    }
                }
                if queued {
                    self.echo(echo, true);
                }
            }
            Action::Info(r) => {
                self.panel = Some(Panel::Region(r));
                self.echo(echo, false);
            }
            Action::Focus(r) => {
                self.camera.focus_region(&self.map, r);
                self.focus_flash = Some((r, Instant::now() + FOCUS_FLASH));
            }
            Action::Status => {
                self.panel = Some(Panel::Status);
                let line = self.status_line();
                self.note(&line, Tone::Text);
            }
            Action::Speed(pct) => {
                self.speed_pct = pct.clamp(SPEED_MIN_PCT, SPEED_MAX_PCT);
                self.acc_us = 0;
                let text = format!("Speed {}", fmt_speed(self.speed_pct));
                self.note(&text, Tone::Accent);
            }
            Action::TogglePause => {
                self.paused = !self.paused;
                self.acc_us = 0;
                if self.paused {
                    self.note("Paused. Press p again to resume.", Tone::Warn);
                } else {
                    self.note("Resumed.", Tone::Accent);
                }
            }
            Action::DefineAlias { name, body } => {
                let text = format!("Alias {} = {}", name, body);
                self.settings.aliases.insert(name, body);
                self.note(&text, Tone::Good);
                if let Some(e) = self.persist_settings() {
                    self.note(&e, Tone::Bad);
                }
            }
            Action::RemoveAlias(name) => {
                self.settings.aliases.remove(&name);
                self.note(&format!("Alias {} removed", name), Tone::Good);
                if let Some(e) = self.persist_settings() {
                    self.note(&e, Tone::Bad);
                }
            }
            Action::ListAliases => {
                self.panel = Some(Panel::Aliases);
            }
            Action::Help(topic) => {
                let lines = help::help_lines(topic.as_deref());
                self.help = Some(HelpOverlay { lines, scroll: 0 });
            }
            Action::Save(name) => {
                let name = match name {
                    Some(n) => n,
                    None => QUICK_SAVE.to_string(),
                };
                let result: Option<Result<String, String>> = match &self.session {
                    Some(s) => match &s.driver {
                        Driver::Live(g) => Some(self.persistence.save_game(&name, g)),
                        Driver::Replay(_) => None,
                    },
                    None => None,
                };
                match result {
                    Some(Ok(path)) => self.note(&format!("Saved '{}' to {}", name, path), Tone::Good),
                    Some(Err(e)) => self.note(&format!("Save failed: {}", e), Tone::Bad),
                    None => self.note("Nothing to save in a replay.", Tone::Warn),
                }
            }
            Action::Quit => {
                let live_running = match &self.session {
                    Some(s) => !s.read_only() && !s.game().is_over(),
                    None => false,
                };
                if live_running {
                    if let Some(s) = self.session.as_mut() {
                        s.abandoned = true;
                    }
                }
                self.enter_game_over();
            }
            Action::Theme(name) => {
                let kind = match name {
                    None => self.settings.theme.next(),
                    Some(n) => match ThemeKind::from_name(&n) {
                        Some(k) => k,
                        None => self.settings.theme,
                    },
                };
                self.apply_theme(kind);
                self.note(&format!("Theme: {}", kind.label()), Tone::Accent);
                if let Some(e) = self.persist_settings() {
                    self.note(&e, Tone::Bad);
                }
            }
        }
    }

    /// The one-line `status` summary for the event log.
    fn status_line(&self) -> String {
        let game = match session_game(&self.session) {
            Some(g) => g,
            None => return String::new(),
        };
        let board = game.leaderboard();
        let rank = board.iter().position(|&p| p == HUMAN).map(|i| i + 1).unwrap_or(0);
        let regions = game.region_count(HUMAN);
        let capital = match game.get_player(HUMAN).and_then(|p| p.capital) {
            Some(r) => format!(" ({})", log::region_name(game, r)),
            None => String::new(),
        };
        format!(
            "{}: {} region{}{}  Pool {}  +{}/s  Land {}%  Rank {}/{}",
            log::player_name(game, HUMAN),
            regions,
            if regions == 1 { "" } else { "s" },
            capital,
            command::fmt_num(game.pool(HUMAN)),
            command::fmt_num(game.growth_per_sec(HUMAN)),
            game.land_pct(HUMAN),
            rank,
            board.len()
        )
    }
}

#[cfg(test)]
mod setup_tests {
    use super::*;

    struct MemoryPersistence;
    impl Persistence for MemoryPersistence {
        fn save_game(&mut self, _: &str, _: &Game) -> Result<String, String> { Ok(String::new()) }
        fn list_saves(&self) -> Vec<String> { Vec::new() }
        fn load_game(&mut self, _: &str) -> Result<Game, String> { Err("No saves".into()) }
        fn save_replay(&mut self, _: &Replay) -> Result<String, String> { Ok(String::new()) }
        fn save_settings(&mut self, _: &Settings) -> Result<(), String> { Ok(()) }
    }

    fn setup() -> App {
        let mut app = App::new(UiOptions {
            settings: Settings::default(),
            start: StartMode::Setup(MatchConfig::default()),
            caps: Capabilities::default(),
            map: WorldMap::builtin(),
            balance: Balance::default(),
            skip_splash: true,
        }, Box::new(MemoryPersistence));
        app.after_splash();
        app
    }

    fn select(app: &mut App, field: Field) {
        app.form.selected = Field::ALL.iter().position(|&f| f == field).unwrap();
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn enter_changes_options_and_only_play_starts_a_match() {
        let mut app = setup();
        select(&mut app, Field::Alliances);
        let before = app.form.alliances;
        press(&mut app, KeyCode::Enter);
        assert_ne!(app.form.alliances, before);
        assert_eq!(app.screen, Screen::Setup);
        assert!(app.session.is_none());
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.form.alliances, before);
        press(&mut app, KeyCode::Right);
        assert_ne!(app.form.alliances, before);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.form.alliances, before);
        select(&mut app, Field::Play);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Countdown);
        assert!(app.session.is_some());
    }

    #[test]
    fn setup_help_preserves_settings_and_never_starts_a_match() {
        let mut app = setup();
        select(&mut app, Field::HowToPlay);
        let config = app.form.clone().build().1;
        press(&mut app, KeyCode::Enter);
        assert!(app.help.is_some());
        press(&mut app, KeyCode::PageDown);
        assert!(app.help.as_ref().unwrap().scroll > 0);
        press(&mut app, KeyCode::Enter);
        assert!(app.help.is_none());
        assert_eq!(app.form.field(), Field::HowToPlay);
        assert_eq!(app.form.clone().build().1, config);
        assert_eq!(app.screen, Screen::Setup);
        assert!(app.session.is_none());
        press(&mut app, KeyCode::Char('h'));
        assert!(app.help.is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.help.is_none());
        assert!(!app.quit);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.form.field(), Field::Play);
    }

    #[test]
    fn setup_buttons_fit_the_minimum_terminal_size() {
        let mut app = setup();
        let backend = ratatui::backend::TestBackend::new(60, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| view::draw(&mut app, frame)).unwrap();
        let text: String = terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("[ PLAY ]"));
        assert!(text.contains("[ HOW TO PLAY ]"));
    }
}
