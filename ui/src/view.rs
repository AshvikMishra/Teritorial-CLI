//! Drawing: one function per screen plus the shared pieces (status bar,
//! leaderboard, event log, hint strip, prompt, panels and overlays).
//!
//! Colors always come from the active [`Theme`], quantized for the terminal's
//! color mode. In ASCII glyph mode every border uses plain `+`, `-` and `|`.

use std::time::Instant;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use territorial_core::{EdgeKind, Game, Outcome, PlayerId, RegionId, HUMAN, TICKS_PER_SECOND};
use territorial_render::{
    art, compact_number, Capabilities, ColorMode, GlyphMode, MapRenderCtx, Rgb, Theme,
};

use crate::app::{session_game, App, Panel, Screen, Session, COUNT_STEP, SPLASH_FRAME};
use crate::command::parser::fmt_speed;
use crate::command::{fmt_num, fmt_ticks};
use crate::log::{self, clock, Tone};
use crate::prompt::Prompt;
use crate::setup::{Field, SetupForm};

/// Below this size the game is not drawn at all.
const MIN_W: u16 = 60;
const MIN_H: u16 = 20;
/// Below this size the playing screen switches to compact mode.
const COMPACT_W: u16 = 100;
const COMPACT_H: u16 = 30;

/// Plain ASCII borders for terminals without Unicode.
const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// The theme's UI colors, quantized for this frame.
struct Pal {
    mode: ColorMode,
    ascii: bool,
    bg: Color,
    text: Color,
    dim: Color,
    accent: Color,
    warn: Color,
    good: Color,
    bad: Color,
    highlight: Color,
}

impl Pal {
    fn new(theme: &Theme, caps: Capabilities) -> Pal {
        let mode = caps.color;
        Pal {
            mode,
            ascii: caps.glyphs == GlyphMode::Ascii,
            bg: theme.color(theme.panel_bg(), mode),
            text: theme.color(theme.text(), mode),
            dim: theme.color(theme.dim(), mode),
            accent: theme.color(theme.accent(), mode),
            warn: theme.color(theme.warn(), mode),
            good: theme.color(theme.good(), mode),
            bad: theme.color(theme.bad(), mode),
            highlight: theme.color(theme.highlight(), mode),
        }
    }

    fn base(&self) -> Style {
        Style::new().fg(self.text).bg(self.bg)
    }

    fn fg(&self, c: Color) -> Style {
        Style::new().fg(c)
    }

    fn tone(&self, theme: &Theme, tone: Tone) -> Style {
        match tone {
            Tone::Text => self.fg(self.text),
            Tone::Dim => self.fg(self.dim),
            Tone::Accent => self.fg(self.accent),
            Tone::Good => self.fg(self.good),
            Tone::Bad => self.fg(self.bad),
            Tone::Warn => self.fg(self.warn),
            Tone::Player(p) => Style::new()
                .fg(theme.player_fg(p, self.mode))
                .add_modifier(Modifier::BOLD),
        }
    }

    fn player(&self, theme: &Theme, p: PlayerId) -> Style {
        Style::new().fg(theme.player_fg(p, self.mode))
    }

    fn rgb(&self, theme: &Theme, rgb: Rgb) -> Color {
        theme.color(rgb, self.mode)
    }

    /// A bordered panel with a title, using ASCII borders when needed.
    fn block(&self, title: &str) -> Block<'static> {
        let title_line = Line::from(Span::styled(
            format!(" {} ", title),
            Style::new().fg(self.accent).add_modifier(Modifier::BOLD),
        ));
        let mut b = Block::new()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(self.dim))
            .style(self.base())
            .title_top(title_line);
        if self.ascii {
            b = b.border_set(ASCII_BORDER);
        }
        b
    }
}

/// Draws the current screen.
pub fn draw(app: &mut App, f: &mut Frame) {
    let area = f.area();
    let pal = Pal::new(&app.theme, app.caps);
    f.render_widget(Block::new().style(pal.base()), area);
    if area.width < MIN_W || area.height < MIN_H {
        draw_too_small(f, area, &pal);
        return;
    }
    let now = Instant::now();
    match app.screen {
        Screen::Splash => draw_splash(app, f, area, &pal, now),
        Screen::Setup => {
            draw_setup(app, f, area, &pal);
            draw_help(app, f, area, &pal);
        },
        Screen::ChooseStart => draw_choose(app, f, area, &pal),
        Screen::Countdown => {
            draw_playing(app, f, area, &pal, now);
            draw_countdown(app, f, area, &pal, now);
        }
        Screen::Playing => draw_playing(app, f, area, &pal, now),
        Screen::GameOver => draw_game_over(app, f, area, &pal),
    }
}

// ----------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------

/// A `w` x `h` rectangle centered in `area` (clipped to it).
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Truncates or pads `s` to exactly `w` characters.
fn fit(s: &str, w: usize) -> String {
    let mut out: String = s.chars().take(w).collect();
    let n = out.chars().count();
    for _ in n..w {
        out.push(' ');
    }
    out
}

/// Truncates `s` to at most `w` characters.
fn trunc(s: &str, w: usize) -> String {
    s.chars().take(w).collect()
}

/// Defense bonus permille as a multiplier, e.g. `x1.50`.
fn fmt_permille(p: u32) -> String {
    format!("x{}.{:02}", p / 1000, (p % 1000) / 10)
}

/// Art as lines in one style.
fn art_lines(text: &'static str, style: Style) -> Vec<Line<'static>> {
    text.lines()
        .map(|l| Line::from(Span::styled(l, style)))
        .collect()
}

fn draw_too_small(f: &mut Frame, area: Rect, pal: &Pal) {
    let lines = vec![
        Line::from(Span::styled(
            "Terminal too small",
            pal.fg(pal.warn).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("{}x{} - need at least {}x{}", area.width, area.height, MIN_W, MIN_H),
            pal.fg(pal.text),
        )),
        Line::from(Span::styled("Ctrl+C quits", pal.fg(pal.dim))),
    ];
    let rect = centered(area, area.width, 3);
    f.render_widget(Paragraph::new(lines).centered().style(pal.base()), rect);
}

// ----------------------------------------------------------------------
// Splash and countdown
// ----------------------------------------------------------------------

fn draw_splash(app: &App, f: &mut Frame, area: Rect, pal: &Pal, now: Instant) {
    let el = now.saturating_duration_since(app.screen_since);
    let step = SPLASH_FRAME.as_millis().max(1);
    let idx = (el.as_millis() / step) as usize;
    let frame = art::splash_frame(idx);
    let (w, h) = art::size(frame);
    if area.width >= w && area.height >= h + 2 {
        let rect = centered(Rect::new(area.x, area.y, area.width, area.height - 2), w, h);
        let theme = &app.theme;
        let mut lines: Vec<Line<'static>> = Vec::new();
        let total = u32::from(h.max(1));
        for (i, l) in frame.lines().enumerate() {
            let pct = ((i as u32) * 100 / total).min(100) as u8;
            let rgb = theme.accent().mix(theme.highlight(), pct);
            lines.push(Line::from(Span::styled(
                l,
                Style::new().fg(pal.rgb(theme, rgb)).add_modifier(Modifier::BOLD),
            )));
        }
        f.render_widget(Paragraph::new(lines), rect);
        let hint_y = (rect.y + rect.height + 1).min(area.y + area.height - 1);
        let hint = Rect::new(area.x, hint_y, area.width, 1);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("press any key", pal.fg(pal.dim)))).centered(),
            hint,
        );
    } else {
        let lines = vec![
            Line::from(Span::styled(
                "TERRITORIAL CLI",
                pal.fg(pal.accent).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(art::TAGLINE, pal.fg(pal.text))),
            Line::from(""),
            Line::from(Span::styled("press any key", pal.fg(pal.dim))),
        ];
        let rect = centered(area, area.width, 4);
        f.render_widget(Paragraph::new(lines).centered(), rect);
    }
}

fn draw_countdown(app: &App, f: &mut Frame, area: Rect, pal: &Pal, now: Instant) {
    let el = now.saturating_duration_since(app.screen_since);
    let step = (el.as_millis() / COUNT_STEP.as_millis().max(1)) as u32;
    let n = 3u32.saturating_sub(step);
    let text = art::countdown(n);
    let (w, h) = art::size(text);
    let rect = centered(area, w + 4, h + 2);
    f.render_widget(Clear, rect);
    let color = if n == 0 { pal.good } else { pal.accent };
    let lines = art_lines(text, Style::new().fg(color).add_modifier(Modifier::BOLD));
    let title = if n == 0 { "GO" } else { "GET READY" };
    f.render_widget(
        Paragraph::new(lines).centered().block(pal.block(title)),
        rect,
    );
}

// ----------------------------------------------------------------------
// Setup
// ----------------------------------------------------------------------

fn draw_setup(app: &App, f: &mut Frame, area: Rect, pal: &Pal) {
    let form: &SetupForm = &app.form;
    let rows_needed: u16 = Field::ALL.iter().filter(|&&field| form.field_visible(field)).count() as u16 + 2;
    let (banner_w, banner_h) = art::size(art::TITLE_BANNER);
    let show_banner = area.width >= banner_w && area.height >= banner_h + rows_needed + 6;
    let top_h = if show_banner { banner_h } else { 1 };
    let chunks = Layout::vertical([
        Constraint::Length(top_h),
        Constraint::Length(1),
        Constraint::Length(rows_needed),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(area);

    if show_banner {
        let rect = centered(chunks[0], banner_w, banner_h);
        let lines = art_lines(
            art::TITLE_BANNER,
            Style::new().fg(pal.accent).add_modifier(Modifier::BOLD),
        );
        f.render_widget(Paragraph::new(lines), rect);
    } else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "TERRITORIAL CLI",
                pal.fg(pal.accent).add_modifier(Modifier::BOLD),
            )))
            .centered(),
            chunks[0],
        );
    }

    let box_w: u16 = 64;
    let form_rect = centered(chunks[2], box_w, rows_needed);
    let label_w: usize = 16;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, field) in Field::ALL.iter().enumerate() {
        if !form.field_visible(*field) {
            continue;
        }
        let selected = i == form.selected;
        if matches!(field, Field::HowToPlay | Field::Play) {
            let style = if selected {
                Style::new().fg(pal.highlight).add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                pal.fg(pal.accent).add_modifier(Modifier::BOLD)
            };
            lines.push(Line::from(Span::styled(
                format!("{}[ {} ]", if selected { "> " } else { "  " }, field.label()), style,
            )));
            continue;
        }
        let value = form.value_text(*field);
        let mut spans: Vec<Span<'static>> = Vec::new();
        let marker = if selected { "> " } else { "  " };
        spans.push(Span::styled(marker, pal.fg(pal.accent)));
        let label_style = if selected {
            pal.fg(pal.text).add_modifier(Modifier::BOLD)
        } else {
            pal.fg(pal.dim)
        };
        spans.push(Span::styled(fit(field.label(), label_w), label_style));
        if selected {
            spans.push(Span::styled(
                format!("< {} >", value),
                Style::new()
                    .fg(pal.highlight)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED),
            ));
            if SetupForm::is_numeric(*field) {
                spans.push(Span::styled("  type digits", pal.fg(pal.dim)));
            }
        } else {
            spans.push(Span::styled(format!("  {}", value), pal.fg(pal.text)));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(
        Paragraph::new(lines).block(pal.block("NEW MATCH")),
        form_rect,
    );

    let help = Line::from(Span::styled(form.field().help(), pal.fg(pal.dim)));
    f.render_widget(Paragraph::new(help).centered(), chunks[3]);

    if let Some((msg, is_err)) = &app.setup_msg {
        let color = if *is_err { pal.bad } else { pal.warn };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(msg.clone(), pal.fg(color)))).centered(),
            chunks[4],
        );
    }

    let keys = Line::from(vec![
        Span::styled("Up/Down", pal.fg(pal.accent)),
        Span::styled(" select  ", pal.fg(pal.dim)),
        Span::styled("Left/Right", pal.fg(pal.accent)),
        Span::styled(" change  ", pal.fg(pal.dim)),
        Span::styled("Enter", pal.fg(pal.accent)),
        Span::styled(" select/change  ", pal.fg(pal.dim)),
        Span::styled("h", pal.fg(pal.accent)),
        Span::styled(" how to play  ", pal.fg(pal.dim)),
        Span::styled("l", pal.fg(pal.accent)),
        Span::styled(" load latest save  ", pal.fg(pal.dim)),
        Span::styled("Esc/q", pal.fg(pal.accent)),
        Span::styled(" quit", pal.fg(pal.dim)),
    ]);
    f.render_widget(Paragraph::new(keys).centered(), chunks[5]);

    let saves_text = match app.saves.first() {
        Some(name) => format!(
            "{} saved match{} - latest: {}",
            app.saves.len(),
            if app.saves.len() == 1 { "" } else { "es" },
            name
        ),
        None => "no saved matches".to_string(),
    };
    let info = Line::from(vec![
        Span::styled(saves_text, pal.fg(pal.dim)),
        Span::styled(
            format!("   renderer: {}", app.caps.describe()),
            pal.fg(pal.dim),
        ),
    ]);
    if chunks[6].height > 0 {
        let r = Rect::new(chunks[6].x, chunks[6].y, chunks[6].width, 1);
        f.render_widget(Paragraph::new(info).centered(), r);
    }
}

// ----------------------------------------------------------------------
// Choose start
// ----------------------------------------------------------------------

fn draw_choose(app: &mut App, f: &mut Frame, area: Rect, pal: &Pal) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    let choose = match &app.choose {
        Some(c) => c,
        None => return,
    };
    let title = Line::from(vec![
        Span::styled(
            "CHOOSE YOUR START",
            pal.fg(pal.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "   {} - {} bots - {}",
                choose.config.preset.name(),
                choose.config.bots,
                choose.config.win.label()
            ),
            pal.fg(pal.dim),
        ),
    ]);
    f.render_widget(Paragraph::new(title), chunks[0]);

    let ctx = MapRenderCtx {
        game: &choose.preview,
        camera: &app.camera,
        theme: &app.theme,
        caps: app.caps,
        viewer: HUMAN,
        highlight: choose.resolved,
        anim_frame: app.anim_frame,
    };
    app.renderer.render(&ctx, chunks[1], f.buffer_mut());

    let shown: Option<RegionId> = match choose.resolved {
        Some(r) => Some(r),
        None => choose.preview.get_player(HUMAN).and_then(|p| p.capital),
    };
    let hint = if let Some(e) = &choose.error {
        Line::from(Span::styled(format!("! {}", e), pal.fg(pal.bad)))
    } else {
        match shown {
            Some(r) => {
                let map = choose.preview.map();
                let info = map.region(r);
                let lead = if choose.resolved.is_some() {
                    "Start in "
                } else {
                    "Enter with an empty line starts in "
                };
                Line::from(vec![
                    Span::styled(lead, pal.fg(pal.dim)),
                    Span::styled(
                        format!("{} ({})", info.name, info.code),
                        pal.fg(pal.good).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            " - {}, value {}, {} neighbors.  Enter confirms, Esc goes back.",
                            info.continent,
                            info.value,
                            map.neighbors(r).len()
                        ),
                        pal.fg(pal.dim),
                    ),
                ])
            }
            None => Line::from(Span::styled(
                "Type a region name or code. Esc goes back.",
                pal.fg(pal.dim),
            )),
        }
    };
    f.render_widget(Paragraph::new(hint), chunks[2]);
    draw_prompt(
        f,
        chunks[3],
        &app.prompt,
        "",
        "type a region to start (name or code)",
        pal,
    );
}

// ----------------------------------------------------------------------
// Playing
// ----------------------------------------------------------------------

/// The region the map should pulse.
fn map_highlight(app: &App, now: Instant) -> Option<RegionId> {
    if let Some((r, _)) = app.focus_flash {
        if app.focus_active(now) {
            return Some(r);
        }
    }
    if app.prompt_target.is_some() {
        return app.prompt_target;
    }
    match &app.panel {
        Some(Panel::Region(r)) => Some(*r),
        _ => None,
    }
}

fn draw_playing(app: &mut App, f: &mut Frame, area: Rect, pal: &Pal, now: Instant) {
    let compact = area.width < COMPACT_W || area.height < COMPACT_H;
    let (status_area, map_area, side_area, log_area, hint_area, prompt_area) = if compact {
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);
        (rows[0], rows[1], None, None, rows[2], rows[3])
    } else {
        let log_h: u16 = if area.height >= 45 { 10 } else { 8 };
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(log_h),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);
        let side_w: u16 = if area.width >= 140 { 34 } else { 30 };
        let cols = Layout::horizontal([Constraint::Min(30), Constraint::Length(side_w)])
            .split(rows[1]);
        (rows[0], cols[0], Some(cols[1]), Some(rows[2]), rows[3], rows[4])
    };

    // The map (needs the renderer mutably).
    let highlight = map_highlight(app, now);
    if let Some(game) = session_game(&app.session) {
        let ctx = MapRenderCtx {
            game,
            camera: &app.camera,
            theme: &app.theme,
            caps: app.caps,
            viewer: HUMAN,
            highlight,
            anim_frame: app.anim_frame,
        };
        app.renderer.render(&ctx, map_area, f.buffer_mut());
    }

    let app: &App = app;
    let session = match &app.session {
        Some(s) => s,
        None => return,
    };
    let game = session.game();

    draw_status(app, session, game, f, status_area, pal, compact);

    if let Some(side) = side_area {
        draw_sidebar(app, game, f, side, pal);
    } else if let Some(panel) = &app.panel {
        let lines = panel_lines(app, game, panel, pal);
        let w = map_area.width.min(36);
        let h = (lines.len() as u16 + 2).min(map_area.height);
        let rect = Rect::new(map_area.x + map_area.width - w, map_area.y, w, h);
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(lines).block(pal.block(panel_title(panel))),
            rect,
        );
    }

    if let Some(log_rect) = log_area {
        draw_log(app, f, log_rect, pal, now);
        draw_hint(app, session, f, hint_area, pal);
    } else if !app.prompt.is_empty() || app.submit_error.is_some() {
        draw_hint(app, session, f, hint_area, pal);
    } else {
        draw_last_log_line(app, f, hint_area, pal, now);
    }

    let placeholder = if session.read_only() {
        "read-only: speed, pause, focus, info, status, help, quit"
    } else {
        "type a command, e.g. a <region> 30%   (help: h)"
    };
    draw_prompt(
        f,
        prompt_area,
        &app.prompt,
        &app.analysis.ghost,
        placeholder,
        pal,
    );

    draw_help(app, f, map_area, pal);
}

fn draw_status(
    app: &App,
    session: &Session,
    game: &Game,
    f: &mut Frame,
    area: Rect,
    pal: &Pal,
    compact: bool,
) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let label = pal.fg(pal.dim);
    let value = pal.fg(pal.text).add_modifier(Modifier::BOLD);
    if !compact {
        spans.push(Span::styled(
            "TERRITORIAL CLI  ",
            pal.fg(pal.accent).add_modifier(Modifier::BOLD),
        ));
    }
    let elapsed = game.elapsed_secs();
    spans.push(Span::styled(clock(elapsed), value));
    if let Some(limit) = game.config().active_time_limit_secs() {
        let rem = game.remaining_secs().unwrap_or(0);
        if compact {
            spans.push(Span::styled(format!(" -{}", clock(rem)), label));
        } else {
            spans.push(Span::styled(
                format!(" / {} ({} left)", clock(u64::from(limit)), clock(rem)),
                label,
            ));
        }
    }
    let alive = game.is_alive(HUMAN);
    if alive {
        spans.push(Span::styled("  Pool ", label));
        let pool = game.pool(HUMAN);
        let pool_text = if compact {
            compact_number(pool)
        } else {
            fmt_num(pool)
        };
        spans.push(Span::styled(pool_text, value));
        spans.push(Span::styled(
            format!(" (+{}/s)", fmt_num(game.growth_per_sec(HUMAN))),
            pal.fg(pal.good),
        ));
        spans.push(Span::styled("  Land ", label));
        spans.push(Span::styled(format!("{}%", game.land_pct(HUMAN)), value));
        spans.push(Span::styled(if compact { "  R " } else { "  Regions " }, label));
        spans.push(Span::styled(game.region_count(HUMAN).to_string(), value));
    } else {
        spans.push(Span::styled(
            "  ELIMINATED",
            pal.fg(pal.bad).add_modifier(Modifier::BOLD),
        ));
    }
    if let territorial_core::WinCondition::Domination { pct } = game.config().win {
        if !compact {
            spans.push(Span::styled(format!("  goal {}%", pct), label));
        }
    }
    spans.push(Span::styled("  ", label));
    if session.is_replay() {
        spans.push(Span::styled(
            "REPLAY ",
            pal.fg(pal.accent).add_modifier(Modifier::BOLD),
        ));
    } else if session.spectating {
        spans.push(Span::styled(
            "SPECTATING ",
            pal.fg(pal.accent).add_modifier(Modifier::BOLD),
        ));
    }
    if app.paused {
        spans.push(Span::styled(
            "PAUSED",
            Style::new()
                .fg(pal.warn)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ));
    } else {
        spans.push(Span::styled(fmt_speed(app.speed_pct), pal.fg(pal.accent)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_sidebar(app: &App, game: &Game, f: &mut Frame, area: Rect, pal: &Pal) {
    let board_area = match &app.panel {
        Some(panel) => {
            let lines = panel_lines(app, game, panel, pal);
            let max_panel = area.height.saturating_sub(8);
            let want = (lines.len() as u16).saturating_add(2);
            let panel_h = want.min(max_panel);
            if panel_h >= 3 {
                let parts = Layout::vertical([Constraint::Min(6), Constraint::Length(panel_h)])
                    .split(area);
                f.render_widget(
                    Paragraph::new(lines).block(pal.block(panel_title(panel))),
                    parts[1],
                );
                parts[0]
            } else {
                area
            }
        }
        None => area,
    };
    let inner_h = board_area.height.saturating_sub(2) as usize;
    let inner_w = board_area.width.saturating_sub(2);
    let lines = leaderboard_lines(app, game, pal, inner_w, inner_h);
    f.render_widget(
        Paragraph::new(lines).block(pal.block("LEADERBOARD")),
        board_area,
    );
}

fn panel_title(panel: &Panel) -> &'static str {
    match panel {
        Panel::Region(_) => "INFO (Esc closes)",
        Panel::Status => "STATUS (Esc closes)",
        Panel::Aliases => "ALIASES (Esc closes)",
    }
}

fn leaderboard_lines(
    app: &App,
    game: &Game,
    pal: &Pal,
    inner_w: u16,
    inner_h: usize,
) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let board = game.leaderboard();
    let mut lines: Vec<Line<'static>> = Vec::new();
    if inner_h == 0 {
        return lines;
    }
    // Reserve one line for the legend when there is room.
    let legend = inner_h >= 6;
    let rows_avail = if legend { inner_h - 1 } else { inner_h };
    let name_w = (inner_w as usize).saturating_sub(13).max(4);

    let mut shown: Vec<(usize, PlayerId)> = Vec::new();
    if board.len() <= rows_avail {
        for (i, p) in board.iter().enumerate() {
            shown.push((i, *p));
        }
    } else {
        let human_pos = board.iter().position(|&p| p == HUMAN);
        let take = rows_avail.saturating_sub(1);
        for (i, p) in board.iter().enumerate().take(take) {
            shown.push((i, *p));
        }
        match human_pos {
            Some(hp) if hp >= take => shown.push((hp, HUMAN)),
            _ => {
                if take < board.len() {
                    shown.push((take, board[take]));
                }
            }
        }
    }

    for (i, p) in shown {
        let alive = game.is_alive(p);
        let mut spans: Vec<Span<'static>> = Vec::new();
        let rank = if alive {
            format!("{:>2} ", i + 1)
        } else {
            " - ".to_string()
        };
        spans.push(Span::styled(rank, pal.fg(pal.dim)));
        let (marker, mstyle) = if p == HUMAN {
            (">", pal.fg(pal.accent).add_modifier(Modifier::BOLD))
        } else if game.are_allied(HUMAN, p) {
            ("*", pal.fg(pal.good).add_modifier(Modifier::BOLD))
        } else if game.pending_proposal(p, HUMAN) {
            ("?", pal.fg(pal.warn).add_modifier(Modifier::BOLD))
        } else {
            (" ", pal.fg(pal.dim))
        };
        spans.push(Span::styled(marker, mstyle));
        let name = fit(&log::player_name(game, p), name_w);
        let name_style = if !alive {
            pal.fg(pal.dim).add_modifier(Modifier::CROSSED_OUT)
        } else if p == HUMAN {
            pal.player(theme, p).add_modifier(Modifier::BOLD)
        } else {
            pal.player(theme, p)
        };
        spans.push(Span::styled(name, name_style));
        let num_style = if alive { pal.fg(pal.text) } else { pal.fg(pal.dim) };
        spans.push(Span::styled(
            format!(" {:>3}%", game.land_pct(p)),
            num_style,
        ));
        spans.push(Span::styled(
            format!(" {:>3}", game.region_count(p)),
            pal.fg(pal.dim),
        ));
        lines.push(Line::from(spans));
    }
    if legend {
        lines.push(Line::from(Span::styled(
            trunc("> you  * ally  ? offers alliance", inner_w as usize),
            pal.fg(pal.dim),
        )));
    }
    lines
}

fn panel_lines(app: &App, game: &Game, panel: &Panel, pal: &Pal) -> Vec<Line<'static>> {
    match panel {
        Panel::Region(r) => region_lines(app, game, *r, pal),
        Panel::Status => status_lines(app, game, pal),
        Panel::Aliases => alias_lines(app, pal),
    }
}

fn kv(key: &str, value: String, pal: &Pal, vstyle: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(fit(key, 10), pal.fg(pal.dim)),
        Span::styled(value, vstyle),
    ])
}

fn owner_span(app: &App, game: &Game, owner: Option<PlayerId>, pal: &Pal) -> Span<'static> {
    match owner {
        Some(p) => Span::styled(log::player_name(game, p), pal.player(&app.theme, p)),
        None => Span::styled("neutral", pal.fg(pal.dim)),
    }
}

fn region_lines(app: &App, game: &Game, r: RegionId, pal: &Pal) -> Vec<Line<'static>> {
    let map = game.map();
    let info = match map.get_region(r) {
        Some(i) => i,
        None => return vec![Line::from("unknown region")],
    };
    let st = game.region_state(r);
    let text = pal.fg(pal.text);
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            info.name.clone(),
            pal.fg(pal.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" ({})", info.code), pal.fg(pal.dim)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(fit("Owner", 10), pal.fg(pal.dim)),
        owner_span(app, game, st.owner, pal),
    ]));
    lines.push(kv(
        "Garrison",
        format!("{} (cap {})", fmt_num(st.garrison), fmt_num(game.garrison_cap(r))),
        pal,
        text,
    ));
    lines.push(kv(
        "Defense",
        format!(
            "{} = {}",
            fmt_permille(game.defense_bonus_permille(r)),
            fmt_num(game.effective_defense(r))
        ),
        pal,
        text,
    ));
    lines.push(kv(
        "Value",
        format!("{}  {}", info.value, info.continent),
        pal,
        text,
    ));
    if st.fortified_until > game.tick() {
        lines.push(kv(
            "Fortified",
            format!("{} left", fmt_ticks(st.fortified_until - game.tick())),
            pal,
            pal.fg(pal.good),
        ));
    }
    for pl in game.players() {
        if pl.capital == Some(r) && pl.alive {
            lines.push(Line::from(vec![
                Span::styled(fit("Capital", 10), pal.fg(pal.dim)),
                Span::styled(pl.name.clone(), pal.player(&app.theme, pl.id)),
            ]));
        }
    }
    let incoming = game.marches_to(r);
    if !incoming.is_empty() {
        let troops: i64 = incoming.iter().map(|m| m.troops).sum();
        lines.push(kv(
            "Incoming",
            format!("{} march(es), {} troops", incoming.len(), fmt_num(troops)),
            pal,
            pal.fg(pal.warn),
        ));
    }
    if game.is_attackable_by(HUMAN, r) {
        lines.push(Line::from(Span::styled(
            "You can attack this region",
            pal.fg(pal.good),
        )));
    }
    lines.push(Line::from(Span::styled("Neighbors:", pal.fg(pal.dim))));
    for &(n, _len, kind) in map.neighbors(r) {
        let ninfo = map.region(n);
        let nst = game.region_state(n);
        let sea = if kind == EdgeKind::Sea { "~" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(format!(" {}", sea), pal.fg(pal.dim)),
            Span::styled(fit(&ninfo.code, 6), pal.fg(pal.text)),
            Span::styled(format!("{:>6} ", compact_number(nst.garrison)), pal.fg(pal.dim)),
            owner_span(app, game, nst.owner, pal),
        ]));
    }
    lines
}

fn status_lines(app: &App, game: &Game, pal: &Pal) -> Vec<Line<'static>> {
    let text = pal.fg(pal.text);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let me = match game.get_player(HUMAN) {
        Some(p) => p,
        None => return lines,
    };
    lines.push(Line::from(Span::styled(
        me.name.clone(),
        pal.player(&app.theme, HUMAN).add_modifier(Modifier::BOLD),
    )));
    let board = game.leaderboard();
    let rank = board.iter().position(|&p| p == HUMAN).map(|i| i + 1).unwrap_or(0);
    lines.push(kv("Rank", format!("{}/{}", rank, board.len()), pal, text));
    lines.push(kv(
        "Pool",
        format!("{} / {}", fmt_num(game.pool(HUMAN)), fmt_num(game.pool_cap(HUMAN))),
        pal,
        text,
    ));
    lines.push(kv(
        "Growth",
        format!("+{}/s", fmt_num(game.growth_per_sec(HUMAN))),
        pal,
        pal.fg(pal.good),
    ));
    lines.push(kv(
        "Land",
        format!("{}%  ({} regions)", game.land_pct(HUMAN), game.region_count(HUMAN)),
        pal,
        text,
    ));
    lines.push(kv("Troops", fmt_num(game.total_troops(HUMAN)), pal, text));
    lines.push(kv("Score", game.score(HUMAN).to_string(), pal, text));
    if let Some(cap) = me.capital {
        lines.push(kv("Capital", log::region_name(game, cap), pal, text));
    }
    let allies = game.allies_of(HUMAN);
    let mut ally_spans: Vec<Span<'static>> = vec![Span::styled(fit("Allies", 10), pal.fg(pal.dim))];
    if allies.is_empty() {
        ally_spans.push(Span::styled("none", pal.fg(pal.dim)));
    } else {
        for (i, a) in allies.iter().enumerate() {
            if i > 0 {
                ally_spans.push(Span::styled(", ", pal.fg(pal.dim)));
            }
            ally_spans.push(Span::styled(log::player_name(game, *a), pal.player(&app.theme, *a)));
        }
    }
    lines.push(Line::from(ally_spans));
    for pr in game.proposals() {
        if pr.to == HUMAN {
            lines.push(Line::from(vec![
                Span::styled("Offer from ", pal.fg(pal.warn)),
                Span::styled(log::player_name(game, pr.from), pal.player(&app.theme, pr.from)),
                Span::styled(
                    format!(" - ally {}", log::player_name(game, pr.from).to_lowercase()),
                    pal.fg(pal.dim),
                ),
            ]));
        }
    }
    let marches: Vec<i64> = game
        .marches()
        .iter()
        .filter(|m| m.owner == HUMAN)
        .map(|m| m.troops)
        .collect();
    if !marches.is_empty() {
        let total: i64 = marches.iter().sum();
        lines.push(kv(
            "Marching",
            format!("{} force(s), {} troops", marches.len(), fmt_num(total)),
            pal,
            text,
        ));
    }
    lines.push(kv(
        "Taken/lost",
        format!("{} / {}", me.stats.regions_captured, me.stats.regions_lost),
        pal,
        text,
    ));
    lines.push(kv("Win", game.config().win.label(), pal, pal.fg(pal.dim)));
    lines.push(Line::from(Span::styled("Rivals:", pal.fg(pal.dim))));
    for p in board.iter().filter(|&&p| p != HUMAN && game.is_alive(p)).take(3) {
        lines.push(Line::from(vec![
            Span::styled("  ", pal.fg(pal.dim)),
            Span::styled(fit(&log::player_name(game, *p), 12), pal.player(&app.theme, *p)),
            Span::styled(
                format!(" {:>3}%  {} regions", game.land_pct(*p), game.region_count(*p)),
                pal.fg(pal.dim),
            ),
        ]));
    }
    lines
}

fn alias_lines(app: &App, pal: &Pal) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if app.settings.aliases.is_empty() {
        lines.push(Line::from(Span::styled("No aliases yet.", pal.fg(pal.dim))));
        lines.push(Line::from(Span::styled(
            "e.g. alias rush a $1 80%",
            pal.fg(pal.accent),
        )));
        return lines;
    }
    for (name, body) in &app.settings.aliases {
        lines.push(Line::from(vec![
            Span::styled(name.clone(), pal.fg(pal.accent).add_modifier(Modifier::BOLD)),
            Span::styled(" = ", pal.fg(pal.dim)),
            Span::styled(body.clone(), pal.fg(pal.text)),
        ]));
    }
    lines
}

fn entry_line(app: &App, e: &log::LogEntry, pal: &Pal, now: Instant) -> Line<'static> {
    let mut extra = Modifier::empty();
    if e.important {
        extra |= Modifier::BOLD;
    }
    if e.flashing(now) {
        extra |= Modifier::REVERSED;
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(
        format!("{:>5} ", clock(e.tick / TICKS_PER_SECOND)),
        pal.fg(pal.dim),
    ));
    for seg in &e.segs {
        spans.push(Span::styled(
            seg.text.clone(),
            pal.tone(&app.theme, seg.tone).add_modifier(extra),
        ));
    }
    Line::from(spans)
}

fn draw_log(app: &App, f: &mut Frame, area: Rect, pal: &Pal, now: Instant) {
    let height = area.height.saturating_sub(2) as usize;
    let entries = app.log.entries();
    let end = entries.len().saturating_sub(app.log.scroll());
    let start = end.saturating_sub(height);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for e in &entries[start..end] {
        lines.push(entry_line(app, e, pal, now));
    }
    let title = if app.log.scroll() > 0 {
        format!("EVENTS (scrolled back {}, PgDn for newer)", app.log.scroll())
    } else {
        "EVENTS".to_string()
    };
    f.render_widget(Paragraph::new(lines).block(pal.block(&title)), area);
}

fn draw_last_log_line(app: &App, f: &mut Frame, area: Rect, pal: &Pal, now: Instant) {
    if let Some(e) = app.log.entries().last() {
        let line = entry_line(app, e, pal, now);
        f.render_widget(Paragraph::new(line), area);
    }
}

fn draw_hint(app: &App, session: &Session, f: &mut Frame, area: Rect, pal: &Pal) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if let Some(err) = &app.submit_error {
        spans.push(Span::styled(
            format!("! {}", err),
            pal.fg(pal.bad).add_modifier(Modifier::BOLD),
        ));
    } else if app.prompt.is_empty() {
        let text = if session.read_only() {
            "Watching. Try: sp 2, p, w <region>, i <region>, s, h, q"
        } else {
            "Examples: a <region> 30%   f <region> 500   a @weakest   ally <bot>   h keys"
        };
        spans.push(Span::styled(text, pal.fg(pal.dim)));
    } else {
        let a = &app.analysis;
        let ghost_ready = !a.ghost.is_empty() && app.prompt.at_end();
        match (&a.ghost_hint, ghost_ready) {
            (Some(gh), true) => {
                spans.push(Span::styled(gh.clone(), pal.fg(pal.text)));
                spans.push(Span::styled("  [Tab]", pal.fg(pal.dim)));
            }
            _ => {
                let summary = a.summary();
                if a.has_errors() {
                    spans.push(Span::styled(summary, pal.fg(pal.bad)));
                } else {
                    spans.push(Span::styled(summary, pal.fg(pal.good)));
                }
            }
        }
        if app.settings.show_hints {
            if let Some(u) = &a.usage {
                spans.push(Span::styled(format!("    {}", u), pal.fg(pal.dim)));
            }
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Draws the `> ` prompt with the cursor, dim ghost text (only when the
/// cursor is at the end) and a dim placeholder when the line is empty.
fn draw_prompt(f: &mut Frame, area: Rect, prompt: &Prompt, ghost: &str, placeholder: &str, pal: &Pal) {
    if area.width < 3 || area.height == 0 {
        return;
    }
    let avail = (area.width - 2) as usize;
    let chars: Vec<char> = prompt.text().chars().collect();
    let len = chars.len();
    let cursor = prompt.cursor().min(len);
    let offset = if cursor >= avail { cursor + 1 - avail } else { 0 };
    let end = len.min(offset + avail);
    let visible: String = chars[offset..end].iter().collect();
    let mut spans: Vec<Span<'static>> = vec![Span::styled(
        "> ",
        pal.fg(pal.accent).add_modifier(Modifier::BOLD),
    )];
    spans.push(Span::styled(visible, pal.fg(pal.text)));
    if prompt.is_empty() {
        spans.push(Span::styled(trunc(placeholder, avail), pal.fg(pal.dim)));
    } else if prompt.at_end() && !ghost.is_empty() {
        let used = end - offset;
        let room = avail.saturating_sub(used);
        if room > 0 {
            spans.push(Span::styled(trunc(ghost, room), pal.fg(pal.dim)));
        }
    }
    let line_area = Rect::new(area.x, area.y, area.width, 1);
    f.render_widget(Paragraph::new(Line::from(spans)), line_area);
    let x = area.x + 2 + (cursor - offset) as u16;
    f.set_cursor_position((x, area.y));
}

fn draw_help(app: &App, f: &mut Frame, area: Rect, pal: &Pal) {
    let help = match &app.help {
        Some(h) => h,
        None => return,
    };
    let w = area.width.saturating_sub(4).clamp(20, 100);
    let want_h = (help.lines.len() as u16).saturating_add(2);
    let h = want_h.min(area.height.saturating_sub(2)).max(3);
    let rect = centered(area, w, h);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, l) in help.lines.iter().enumerate() {
        let style = if i == 0 {
            pal.fg(pal.accent).add_modifier(Modifier::BOLD)
        } else if l.starts_with("  ") {
            pal.fg(pal.text)
        } else {
            pal.fg(pal.accent)
        };
        lines.push(Line::from(Span::styled(l.clone(), style)));
    }
    let scroll = help.scroll.min(u16::MAX as usize) as u16;
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .block(pal.block("HELP - Esc closes, PgUp/PgDn scroll"))
            .scroll((scroll, 0)),
        rect,
    );
}

// ----------------------------------------------------------------------
// Game over
// ----------------------------------------------------------------------

fn draw_game_over(app: &App, f: &mut Frame, area: Rect, pal: &Pal) {
    let session = match &app.session {
        Some(s) => s,
        None => return,
    };
    let game = session.game();
    let outcome = game.outcome();
    let won = match outcome {
        Some(o) => o.is_winner(HUMAN),
        None => false,
    };
    let allied = matches!(outcome, Some(Outcome::AlliedVictory { .. }));
    let can_watch =
        session.human_dead && !session.abandoned && !session.is_replay() && !game.is_over();
    // A bot-only match (`--bot-only-watch`): player 0 was a bot, not the viewer.
    let bot_only = game.config().spectate;

    let (art_text, headline, good): (&'static str, String, bool) = if session.abandoned {
        (art::DEFEAT, "YOU LEFT THE MATCH".to_string(), false)
    } else if outcome.is_some() {
        let prefix = if session.is_replay() { "REPLAY: " } else { "" };
        if bot_only {
            (art::VICTORY, format!("{}MATCH OVER", prefix), true)
        } else if won && allied {
            (art::VICTORY, format!("{}ALLIED VICTORY", prefix), true)
        } else if won {
            (art::VICTORY, format!("{}VICTORY", prefix), true)
        } else {
            (art::DEFEAT, format!("{}DEFEAT", prefix), false)
        }
    } else if session.is_replay() {
        (art::DEFEAT, "REPLAY FINISHED".to_string(), false)
    } else if session.human_dead {
        (art::DEFEAT, "DEFEAT - YOU WERE ELIMINATED".to_string(), false)
    } else {
        (art::DEFEAT, "MATCH ENDED".to_string(), false)
    };

    let mut sub: Vec<Line<'static>> = Vec::new();
    if let Some(o) = outcome {
        sub.push(Line::from(Span::styled(
            log::outcome_text(game, o),
            pal.fg(pal.text).add_modifier(Modifier::BOLD),
        )));
    }
    if let Some(me) = game.get_player(HUMAN).filter(|_| !bot_only) {
        if let Some(at) = me.stats.eliminated_at {
            let mut spans = vec![Span::styled(
                format!("You were eliminated at {}", clock(at / TICKS_PER_SECOND)),
                pal.fg(pal.bad),
            )];
            if let Some(by) = me.stats.eliminated_by {
                spans.push(Span::styled(" by ", pal.fg(pal.bad)));
                spans.push(Span::styled(log::player_name(game, by), pal.player(&app.theme, by)));
            }
            sub.push(Line::from(spans));
        }
    }
    sub.push(Line::from(Span::styled(
        format!(
            "Match time {}  -  seed {}  -  {}",
            clock(game.elapsed_secs()),
            game.config().seed,
            game.config().win.label()
        ),
        pal.fg(pal.dim),
    )));
    match &session.replay_path {
        Some(Ok(path)) => sub.push(Line::from(Span::styled(
            format!("Replay saved: {}", path),
            pal.fg(pal.dim),
        ))),
        Some(Err(e)) => sub.push(Line::from(Span::styled(
            format!("Replay not saved: {}", e),
            pal.fg(pal.bad),
        ))),
        None => {}
    }

    let mut keys = vec![
        Span::styled("Enter", pal.fg(pal.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" new match   ", pal.fg(pal.dim)),
        Span::styled("q", pal.fg(pal.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" quit", pal.fg(pal.dim)),
    ];
    if can_watch {
        keys.push(Span::styled("   w", pal.fg(pal.accent).add_modifier(Modifier::BOLD)));
        keys.push(Span::styled(" watch the bots finish", pal.fg(pal.dim)));
    }

    let (art_w, art_h) = art::size(art_text);
    let players = game.players().len() as u16;
    let table_h = players.saturating_add(3);
    let fixed_h = 1 + sub.len() as u16 + 1 + 1;
    let show_art = area.width >= art_w && area.height >= art_h + fixed_h + table_h.min(10);
    let art_rows = if show_art { art_h } else { 0 };

    let chunks = Layout::vertical([
        Constraint::Length(art_rows),
        Constraint::Length(1),
        Constraint::Length(sub.len() as u16),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .split(area);

    let color = if good { pal.good } else { pal.bad };
    if show_art {
        let rect = centered(chunks[0], art_w, art_h);
        f.render_widget(
            Paragraph::new(art_lines(art_text, Style::new().fg(color))),
            rect,
        );
    }
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            headline,
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        )))
        .centered(),
        chunks[1],
    );
    f.render_widget(Paragraph::new(sub).centered(), chunks[2]);

    let table_area = chunks[3];
    let table_w = table_area.width.min(110);
    let rect = Rect::new(
        table_area.x + (table_area.width - table_w) / 2,
        table_area.y,
        table_w,
        table_area.height,
    );
    let inner_h = rect.height.saturating_sub(2) as usize;
    let lines = stats_lines(app, game, pal, rect.width.saturating_sub(2), inner_h);
    f.render_widget(
        Paragraph::new(lines).block(pal.block("FINAL STANDINGS")),
        rect,
    );
    f.render_widget(Paragraph::new(Line::from(keys)).centered(), chunks[4]);
}

fn stats_lines(
    app: &App,
    game: &Game,
    pal: &Pal,
    inner_w: u16,
    inner_h: usize,
) -> Vec<Line<'static>> {
    let wide = inner_w >= 100;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if inner_h == 0 {
        return lines;
    }
    let header = if wide {
        format!(
            "{:>3}  {}{:>5} {:>8} {:>5} {:>9} {:>5} {:>12} {:>12}  {}",
            "#", fit("Player", 16), "Land", "Regions", "Peak", "Captured", "Lost", "Troops sent",
            "Troops lost", "Status"
        )
    } else {
        format!(
            "{:>3}  {}{:>5} {:>8} {:>9} {:>5}  {}",
            "#", fit("Player", 14), "Land", "Regions", "Captured", "Lost", "Status"
        )
    };
    lines.push(Line::from(Span::styled(
        trunc(&header, inner_w as usize),
        pal.fg(pal.dim).add_modifier(Modifier::BOLD),
    )));
    let board = game.leaderboard();
    let rows_avail = inner_h.saturating_sub(1);
    let human_pos = board.iter().position(|&p| p == HUMAN);
    let mut shown: Vec<(usize, PlayerId)> = Vec::new();
    for (i, p) in board.iter().enumerate() {
        if shown.len() + 1 >= rows_avail && board.len() > rows_avail {
            break;
        }
        shown.push((i, *p));
    }
    if let Some(hp) = human_pos {
        if !shown.iter().any(|&(_, p)| p == HUMAN) && rows_avail > 0 {
            shown.push((hp, HUMAN));
        }
    }
    shown.truncate(rows_avail);

    let outcome = game.outcome();
    for (i, p) in shown {
        let pl = match game.get_player(p) {
            Some(pl) => pl,
            None => continue,
        };
        let st = &pl.stats;
        let winner = match outcome {
            Some(o) => o.is_winner(p),
            None => false,
        };
        let status = if winner {
            "WINNER".to_string()
        } else if let Some(at) = st.eliminated_at {
            let by = match st.eliminated_by {
                Some(b) => format!(" by {}", log::player_name(game, b)),
                None => String::new(),
            };
            format!("out {}{}", clock(at / TICKS_PER_SECOND), by)
        } else if pl.alive {
            "alive".to_string()
        } else {
            "out".to_string()
        };
        let name_w = if wide { 16 } else { 14 };
        let name = fit(&pl.name, name_w);
        let body = if wide {
            format!(
                "{:>5} {:>8} {:>5} {:>9} {:>5} {:>12} {:>12}  ",
                format!("{}%", game.land_pct(p)),
                game.region_count(p),
                st.peak_regions,
                st.regions_captured,
                st.regions_lost,
                compact_number(st.troops_sent.min(i64::MAX as u64) as i64),
                compact_number(st.troops_lost.min(i64::MAX as u64) as i64),
            )
        } else {
            format!(
                "{:>5} {:>8} {:>9} {:>5}  ",
                format!("{}%", game.land_pct(p)),
                game.region_count(p),
                st.regions_captured,
                st.regions_lost,
            )
        };
        let mut name_style = pal.player(&app.theme, p);
        if p == HUMAN {
            name_style = name_style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
        }
        let status_style = if winner {
            pal.fg(pal.good).add_modifier(Modifier::BOLD)
        } else if pl.alive {
            pal.fg(pal.text)
        } else {
            pal.fg(pal.dim)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{:>3}  ", i + 1), pal.fg(pal.dim)),
            Span::styled(name, name_style),
            Span::styled(body, pal.fg(pal.text)),
            Span::styled(status, status_style),
        ]));
    }
    lines
}
