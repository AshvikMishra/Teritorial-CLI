# Territorial CLI

A real-time territory conquest game for the terminal, inspired by territorial.io.
You start with one region of a colored world map, grow a national troop pool, and
conquer your neighbors faster than up to 40 bots. There is no mouse and no hotkey
spam: every move is a typed command such as `a fr 40%`. Your typing speed,
command fluency and map knowledge decide how good you are.

- About 214 regions (countries, with large ones split into parts like `us-w` and
  `ru-e`), connected by land borders and curated sea lanes.
- Troops march along graph edges, so attacks take time and the defender can react.
- Bots have four personalities (aggressor, turtle, opportunist, warlord) and
  three difficulties. They use the same command API you do, with limited reaction speed.
- Alliances, truces, save and resume, and deterministic replays.
- Half-block truecolor map, with 256-color and ASCII fallbacks, plus dark,
  high-contrast and colorblind-safe themes.

> **Status: not compiled yet.** The code was written on a machine that had no
> Rust toolchain installed, so it has never been built. Your first `cargo build`
> may report errors, which should be small fixes. Please report or fix them.

## Build and run

You need Rust 1.74 or newer ([rustup.rs](https://rustup.rs)). The map assets are
already in `assets/` and are embedded in the binary, so you need neither Python
nor network access.

```sh
cargo run --release -p territorial                 # splash, then the setup screen
cargo run --release -p territorial -- --quick      # start a match right away
cargo build --release                              # binary at target/release/territorial
```

Use a terminal of at least 100x30 (160x50 or more is recommended). Below 100x30
the game switches to a compact layout. Below 60x20 it asks you to enlarge the window.

## Command-line flags

| Flag | Meaning |
| --- | --- |
| `--preset blitz\|standard\|epic\|custom` | Match preset (see [Match presets](#match-presets)) |
| `--minutes N` | Custom match length in minutes (0 means no timer). Sets the preset to custom and adjusts growth to fit. |
| `--bots N` | Number of bots, 3 to 40 |
| `--difficulty easy\|normal\|hard\|mixed` | Bot difficulty |
| `--win domination\|elimination\|timed` | Win condition |
| `--domination-pct N` | Share of world value needed for a domination win (10 to 100, default 60). On its own it also selects domination. |
| `--seed N` | Fixed RNG seed. Without it the seed is random. |
| `--start <region>` | Start region, as a code (`fr`, `us-w`) or a name (`France`, `south-africa`). Starts the match right away. |
| `--no-alliances` | Disable alliances |
| `--theme dark\|high-contrast\|colorblind` | Color theme (`hc` and `cb` also work) for this run only. The saved setting is kept unless you pick another theme in the game. |
| `--renderer auto\|truecolor\|256\|ascii` | Map renderer for this run only. The saved setting is kept unless you pick another renderer in the game. |
| `--balance <file>` | Balance TOML that overrides the built-in values (see [Balance tuning](#balance-tuning)) |
| `--load <save>` | Resume a saved match, given as a save name or a path to a `.json` file |
| `--replay <file>` | Watch a replay (read-only), given as a path or a name in the replays folder |
| `--quick` | Skip the splash and setup screens and start immediately |
| `--bot-only-watch` | Watch a match played only by bots in the TUI. Player 0 is also a bot (`Bot-0`). The prompt is read-only: speed, pause, focus, info, status, help, theme and quit work. |
| `--simulate` | Run a bot-only match headless (no TUI) and print the progress and result |
| `--max-ticks N` | With `--simulate`, give up after N ticks (10 ticks = 1 s). The default is the timer plus 2 minutes, or 60 minutes when there is no timer. |
| `--version`, `--help` | Version and help |

How the match configuration is built:

- With no flags you get the setup screen, prefilled from the last match you
  started there. Otherwise it uses the standard preset.
- Match flags (`--preset`, `--bots` and so on) change those prefilled values.
  With `--quick`, `--start` or `--bot-only-watch`, the match starts right away
  with that configuration.
- `--simulate` always starts from the standard preset plus your flags, so a
  run can be reproduced from its command line alone, for example
  `territorial --simulate --preset blitz --bots 12 --difficulty hard --seed 42`.
- Match flags are ignored with `--load` and `--replay`, and you get a warning.

Exit codes:

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | Terminal or I/O error |
| 2 | Invalid flags, invalid match settings, or a bad balance, save or replay file |
| 3 | `--simulate` reached `--max-ticks` without a result |

## How to play

Everything happens at the always-on prompt at the bottom of the screen. Start
typing; there is no mode to switch into. The hint strip above the prompt shows
what the line will do before you press Enter, for example
`ATTACK Brazil (garrison 120) send 4,200 troops, ETA 8s`. Invalid commands stay
on the line with the reason, so you can fix them.

- Your **pool** grows over time. Growth depends on the value of the land you
  own, and it slows as the pool nears its cap.
- **Attack** an adjacent region, meaning one that shares a land border or sea
  lane with your territory. The troops leave your pool and march. They capture
  the region if they outnumber its garrison times its defense bonus. Capitals
  and fortified regions defend better, and neutral regions get a small bonus.
- **Fortify** moves pool troops into a garrison and gives it a temporary defense
  bonus. **Recall** pulls garrison troops back into the pool after a short delay.
- To win you need **domination** (a share of world value, 60% by default),
  **elimination** (last player or alliance standing), or the highest **score**
  when the timer runs out.

### Command cheat sheet

| Verb | Short | Example | Effect |
| --- | --- | --- | --- |
| `attack <target> [amount]` | `a` | `a br 40%` | Send 40% of your pool toward Brazil |
| `fortify <region> <amount>` | `f` | `f us-w 500` | Move 500 troops into a garrison |
| `recall <region> <amount>` | `r` | `r fr 200` | Pull troops back into the pool |
| `ally <player>` | `al` | `al bot-3` | Propose an alliance, or accept one |
| `unally <player>` | `ua` | `ua bot-3` | Break an alliance. A truce follows. |
| `info [region]` | `i` | `i de` | Region details and neighbors |
| `focus [region]` | `w` | `w jp` | Move the camera |
| `status` | `s` | `s` | Your stats and rivals |
| `speed [x]` | `sp` | `sp 2` | Game speed from 0.25x to 8x (`sp` alone resets it) |
| `pause` | `p` | `p` | Pause or resume |
| `alias [name [body]]` |  | `alias rush a $1 80%` | Define a shortcut, or list them |
| `unalias <name>` |  | `unalias rush` | Remove a shortcut |
| `save [name]` | `sv` | `sv epic1` | Save the match (no name means quick save) |
| `theme [name]` |  | `theme cb` | Switch the theme (`dark`, `hc`, `cb`) |
| `help [topic]` | `h` | `h targets` | Help. Topics are a verb, `targets`, `amounts`, `chain` and `keys`. |
| `quit` | `q` | `q` | Leave the match. The replay is still saved. |

A unique prefix of a verb also works, for example `att`.

**Targets.** Regions can be given as a code (`fr`, `us-w`), a name (`france`,
`south africa`), a unique prefix (`fra`) or a near miss (`frnace`). Case doesn't matter.
Smart targets save typing under pressure:

- `@weakest` (`@w`) is the weakest enemy region bordering you. When no enemy borders you, it is the weakest neutral one.
- `@richest` (`@r`) is the highest-value region you can attack.
- `@player:<name>` (`@p:<name>`) is that player's nearest region.

Players are named by name (`bot-3`), unique prefix or number (`3`).

**Amounts.**

- `40%` is a percent of your pool. For recall, it is a percent of the movable garrison.
- `500`, `4,200` and `1.2k` are flat troop counts.
- `half` means 50%, and `all` or `max` means 100%.
- An attack with no amount uses your default percent: 25% against neutral regions and 40% against enemies. Both are in the settings.
- The amount may come before the target, as in `a 40% fr`.

**Chaining, repeat and history.**

- `a @weakest 25% ; f es 200` runs several commands at once.
- `!!` repeats the last line, and `!! 60%` repeats it with text appended.
- Up and Down browse the history. Esc clears the line.

**Aliases.**

- `alias rush a $1 80%` makes `rush fr` mean `a fr 80%`. `$1` to `$9` are the
  arguments and `$*` is all of them. If the body has no placeholders, the
  arguments are appended to it.
- Quote a body that contains `;`, as in `alias go "a @weakest 30%; f es 10%"`.
- Aliases are saved in the settings.

### Keys

| Key | Action |
| --- | --- |
| Enter / Esc | Submit / clear the line |
| Tab | Accept the dimmed completion. With an empty prompt, cycle the camera through your regions (Shift+Tab goes backwards). |
| Up / Down | Command history |
| Left, Right, Home, End, Backspace, Delete | Edit the line. With an empty prompt, Left and Right pan the map. |
| Ctrl+U / Ctrl+W / Ctrl+A / Ctrl+E | Clear the line / delete a word / go to the start / go to the end |
| Shift or Ctrl + arrows | Pan the map |
| `+` / `-` | Zoom in or out through seven levels (only when the prompt is empty, because region codes contain `-`) |
| PageUp / PageDown | Scroll the event log or the help page |
| Ctrl+C | Quit immediately |

Characters typed with AltGr (such as `@` on many European keyboard layouts on
Windows, which reports AltGr as Ctrl+Alt) go into the prompt as usual.

On the setup screen:

- Up and Down (or Tab) select a field. Left, Right and Space change it, and digits type numbers.
- Enter starts the match, `l` loads the newest save, and Esc or `q` quits.

On the game-over screen:

- Enter starts a new match and `q` quits.
- `w` keeps watching the bots after you have been eliminated.

Bot decision intervals are 20–32 seconds on Easy, 10–16 seconds on Normal,
and 4–7 seconds on Hard, measured in game time. The opening move uses the same
interval. Easy makes more mistakes; Hard still uses more advanced tactics.

## Match presets

| Preset | Timer | Pool growth | Bots |
| --- | --- | --- | --- |
| blitz | 8 min | 200% | 8 |
| standard | 25 min | 100% | 12 |
| epic | 75 min | 60% | 20 |
| custom | any (`--minutes`) | scaled to the length | 12 |

All presets default to domination at 60% with normal bots and alliances on.
For domination and elimination, preset minutes control growth pacing only; there is no countdown or time limit.
Custom lengths scale the growth rate so that shorter matches grow faster. The
curve runs through 250% at 5 min, 200% at 8, 100% at 25, 60% at 75 and 50% at
120 or more. Only Timed matches end at the time limit.

## Saves, replays and settings

| What | Linux and macOS | Windows |
| --- | --- | --- |
| Settings | `$XDG_CONFIG_HOME/territorial/settings.json` (default `~/.config/territorial/settings.json`) | `%APPDATA%\territorial\settings.json` |
| Saves | `$XDG_DATA_HOME/territorial/saves/<name>.json` (default `~/.local/share/territorial/saves/`) | `%APPDATA%\territorial\saves\` |
| Replays | `$XDG_DATA_HOME/territorial/replays/replay-YYYYMMDD-HHMMSS.json` | `%APPDATA%\territorial\replays\` |

- The folders are created the first time something is written. All files are JSON.
- `save` / `sv` writes a save. Names are reduced to letters, digits, `-` and `_`.
  `sv` with no name writes `quicksave`.
- Resume a save with `--load quicksave`, `--load epic1` or `--load path/to/file.json`,
  or press `l` on the setup screen to load the newest one. A save is checked
  against the built-in world map and its balance values when it is loaded; a
  save that does not fit is rejected with an error instead of being resumed.
- A replay is saved automatically, once per match, when a live match ends,
  including when you quit. If you are eliminated, it is saved at that moment,
  and watching the bots finish afterwards does not write a second file. It holds the config, balance, seed and your command log. The bots
  regenerate their moves deterministically. Watch it with `--replay <file>`.
- Settings hold the theme, renderer, default attack percents, aliases, history
  size, hints and the last match configuration. If the settings file is
  corrupt, the game warns you, copies it to `settings.json.bak` and starts
  with defaults.

## Balance tuning

Every gameplay number lives in `assets/balance.toml`. That file is embedded in
the binary, and every value in it is an integer with a comment. To experiment,
copy only the keys you want to change into a new file:

```toml
# fast.toml
growth_base_milli = 9000
neutral_defense_permille = 1000
march_ticks_per_1000km = 40
```

```sh
territorial --balance fast.toml
territorial --simulate --balance fast.toml --seed 7      # check that bot matches still finish
```

- Keys you leave out keep their built-in values.
- Unknown keys are an error, and the error suggests the nearest valid key.
- Values that would break the simulation, such as zero divisors or inverted
  ranges, are rejected with exit code 2.
- `Balance::default()` in `core/src/balance.rs` must stay equal to `assets/balance.toml`.

`--simulate` is the quickest way to tune. It prints a line every simulated
minute with the alive players and the top three by land share, then the
outcome, the winner's personality and difficulty, and the wall time.

## Regenerating the map

The map comes from Natural Earth (public domain) through `mapgen/mapgen.py`. It
is a Python 3.10+ script that uses only the standard library. Its output
(`assets/world_regions.json` and `assets/world_bitmap.bin`) is committed, so
you only need it to change the map:

```sh
python3 -I mapgen/mapgen.py --download --out assets                  # fetches the 1:50m countries into mapgen/cache/
python3 -I mapgen/mapgen.py --input ne_50m_admin_0_countries.geojson --out assets
python3 -I mapgen/mapgen.py --input ... --out assets --preview       # ASCII preview
```

The hand-tuned data lives in `mapgen/curation.json`: merges, splits, sea lanes
and value overrides. See `mapgen/README.md` for the pipeline and
`docs/ARCHITECTURE.md` section 2 for the file formats.

## Project layout

```
Cargo.toml        workspace: core, render, ui, cli
assets/           world_regions.json, world_bitmap.bin, balance.toml (embedded at build time)
core/             territorial-core: deterministic simulation, rules, map graph, bots, saves (no I/O)
render/           territorial-render: map rasterizer, camera, themes, capability detection, ASCII art
ui/               territorial-ui: ratatui screens, layout, command parser, completion, input
cli/              territorial (the binary): flags, settings, save and replay files, headless --simulate
mapgen/           mapgen.py + curation.json: Natural Earth -> assets/
docs/             ARCHITECTURE.md: the cross-crate contracts
```

The simulation core is deterministic. It uses integer math, ordered containers
and its own seeded RNG, and it never reads the clock. The same config, balance,
seed and command log always produce the same match. This is what makes replays
possible, and it keeps the door open for online play later.
