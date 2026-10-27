//! `--simulate`: a headless, bot-only match that prints its progress and result.

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use territorial_core::config::DEFAULT_SEED;
use territorial_core::{
    Balance, EventKind, Game, MatchConfig, Outcome, PlayerId, PlayerKind, WinReason, WorldMap,
    TICKS_PER_SECOND,
};
use territorial_render::grouped_number;

use crate::args::Cli;
use crate::match_config::{apply_flags, time_seed};
use crate::text::clock;
use crate::{Failure, EXIT_STALL};

/// Extra ticks allowed past the timer before declaring a stall.
const TIMER_MARGIN_TICKS: u64 = 120 * TICKS_PER_SECOND;
/// Tick budget for matches without a timer (60 minutes).
const NO_TIMER_TICKS: u64 = 60 * 60 * TICKS_PER_SECOND;
/// Progress is printed once per simulated minute.
const PROGRESS_EVERY_TICKS: u64 = 60 * TICKS_PER_SECOND;

/// Runs the headless match. Exit code 0 when the match is decided, 3 when
/// `--max-ticks` is reached first.
pub fn run(cli: &Cli, map: Arc<WorldMap>, balance: Balance) -> Result<ExitCode, Failure> {
    let mut cfg = apply_flags(MatchConfig::default(), cli, &map)?;
    cfg.spectate = true;
    cfg.human_name = "Bot-0".to_string();
    if cli.seed.is_none() && cfg.seed == DEFAULT_SEED {
        cfg.seed = time_seed();
    }
    cfg.validate(&map)
        .map_err(|e| Failure::usage(format!("invalid match settings: {}", e)))?;

    let max_ticks: u64 = match cli.max_ticks {
        Some(n) => n.max(1),
        None => match cfg.time_limit_ticks() {
            Some(t) => t + TIMER_MARGIN_TICKS,
            None => NO_TIMER_TICKS,
        },
    };

    let timer = match cfg.time_limit_secs {
        Some(s) => clock(u64::from(s)),
        None => "none".to_string(),
    };
    out(&format!(
        "Simulating a {} match: {} bots + Bot-0 ({}), {}, timer {}, alliances {}, seed {}",
        cfg.preset.name(),
        cfg.bots,
        cfg.difficulty.name(),
        cfg.win.label(),
        timer,
        if cfg.alliances { "on" } else { "off" },
        cfg.seed
    ));
    out(&format!(
        "Map: {} regions. Giving up after {} ticks ({}).",
        map.len(),
        grouped_number(max_ticks as i64),
        clock(max_ticks / TICKS_PER_SECOND)
    ));

    let started = Instant::now();
    let mut game = Game::new(Arc::clone(&map), balance, cfg);
    while !game.is_over() && game.tick() < max_ticks {
        let events = game.step();
        for ev in &events {
            if let EventKind::Eliminated { player, by } = &ev.kind {
                let by_text = match by {
                    Some(b) => format!(" by {}", player_name(&game, *b)),
                    None => String::new(),
                };
                out(&format!(
                    "[{:>7}] {} was eliminated{}",
                    clock(ev.tick / TICKS_PER_SECOND),
                    player_name(&game, *player),
                    by_text
                ));
            }
        }
        if game.tick() % PROGRESS_EVERY_TICKS == 0 && !game.is_over() {
            print_progress(&game);
        }
    }
    let wall = started.elapsed();

    out("");
    let code = match game.outcome() {
        Some(outcome) => {
            print_outcome(&game, outcome);
            ExitCode::SUCCESS
        }
        None => {
            out(&format!(
                "STALL: no result after {} ticks ({} of game time).",
                grouped_number(game.tick() as i64),
                clock(game.elapsed_secs())
            ));
            ExitCode::from(EXIT_STALL)
        }
    };
    print_standings(&game);

    let secs = wall.as_secs_f64();
    let rate = if secs > 0.0 {
        format!("{:.0} ticks/s", game.tick() as f64 / secs)
    } else {
        "instant".to_string()
    };
    out(&format!(
        "Simulated {} ticks ({} of game time) in {:.2} s wall time ({}).",
        grouped_number(game.tick() as i64),
        clock(game.elapsed_secs()),
        secs,
        rate
    ));
    Ok(code)
}

/// One progress line: time, tick, players alive, top 3 by land share.
fn print_progress(game: &Game) {
    let alive = game.alive_players();
    let top: Vec<String> = by_land(game, &alive)
        .into_iter()
        .take(3)
        .map(|p| format!("{} {}%", player_name(game, p), game.land_pct(p)))
        .collect();
    out(&format!(
        "[{:>7}] tick {:>6}  alive {:>2}/{}  top: {}",
        clock(game.elapsed_secs()),
        game.tick(),
        alive.len(),
        game.players().len(),
        top.join(", ")
    ));
}

/// The result line plus details about the winner(s).
fn print_outcome(game: &Game, outcome: &Outcome) {
    let at = clock(game.elapsed_secs());
    match outcome {
        Outcome::Winner { player, reason } => {
            let why = match reason {
                WinReason::Domination => "domination",
                WinReason::Elimination => "elimination",
                WinReason::Timer => "highest score at the timer",
            };
            out(&format!(
                "RESULT: {} wins by {} at {} (tick {}).",
                player_name(game, *player),
                why,
                at,
                game.tick()
            ));
            out(&format!("Winner: {}", describe_player(game, *player)));
        }
        Outcome::AlliedVictory { players, top } => {
            let names: Vec<String> = players.iter().map(|&p| player_name(game, p)).collect();
            out(&format!(
                "RESULT: allied victory of {} at {} (tick {}).",
                names.join(", "),
                at,
                game.tick()
            ));
            out(&format!("Top ally: {}", describe_player(game, *top)));
        }
    }
}

/// The five best players by land share (alive first).
fn print_standings(game: &Game) {
    let all: Vec<PlayerId> = game.players().iter().map(|pl| pl.id).collect();
    out("Final standings:");
    for (i, p) in by_land(game, &all).into_iter().take(5).enumerate() {
        let status = if game.is_alive(p) { "" } else { "  (eliminated)" };
        out(&format!(
            "  {}. {:<8} {:>3}% land  {:>3} regions  score {}{}",
            i + 1,
            player_name(game, p),
            game.land_pct(p),
            game.region_count(p),
            grouped_number(game.score(p) as i64),
            status
        ));
    }
}

/// Players sorted alive first, then land share, score and id.
fn by_land(game: &Game, players: &[PlayerId]) -> Vec<PlayerId> {
    let mut rows: Vec<(bool, u32, u64, PlayerId)> = players
        .iter()
        .map(|&p| (game.is_alive(p), game.land_pct(p), game.score(p), p))
        .collect();
    rows.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then(b.2.cmp(&a.2))
            .then(a.3.cmp(&b.3))
    });
    rows.into_iter().map(|row| row.3).collect()
}

/// `Bot-3, aggressor / hard, 61% land, 54 regions, peak 57`.
fn describe_player(game: &Game, p: PlayerId) -> String {
    let style = match game.get_player(p) {
        Some(pl) => match pl.kind {
            PlayerKind::Bot {
                personality,
                difficulty,
            } => format!("{} / {}", personality.name(), difficulty.name()),
            PlayerKind::Human => "human".to_string(),
        },
        None => "unknown".to_string(),
    };
    let peak = match game.get_player(p) {
        Some(pl) => pl.stats.peak_regions,
        None => 0,
    };
    format!(
        "{}, {}, {}% land, {} regions (peak {}), score {}",
        player_name(game, p),
        style,
        game.land_pct(p),
        game.region_count(p),
        peak,
        grouped_number(game.score(p) as i64)
    )
}

fn player_name(game: &Game, p: PlayerId) -> String {
    match game.get_player(p) {
        Some(pl) => pl.name.clone(),
        None => format!("player {}", p),
    }
}

/// Prints a line to stdout, ignoring a closed pipe.
fn out(line: &str) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{}", line);
}
