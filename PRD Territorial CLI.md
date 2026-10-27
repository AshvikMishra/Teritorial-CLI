# PRD: Territorial CLI

*Working title. A real-time, terminal-native territory conquest game where you play entirely by typing commands.*

**Status:** Draft v1\
**Owner:** Ashvik\
**Stack decision:** Rust, ratatui, crossterm

## 1. Summary

Territorial CLI is a real-time strategy game inspired by territorial.io, played in the terminal on a world map. You start with one region, grow a national troop pool, and conquer neighbors faster than the bots do. There is no mouse and no hotkey spam. Every action is a typed command, so your typing speed, command fluency and map knowledge become the skill ceiling. The v1 opponents are bots. Online multiplayer is a planned later phase, and the architecture is built for it from day one.

## 2. Goals and Non-Goals

### Goals

- A fast, light, good-looking game that runs smoothly in any modern terminal.
- A command system that feels like a fast game console, not a chore.
- Bots that feel like distinct opponents, and play by the same rules and command API as the player.
- Configurable match length, from a 5 minute blitz to a long campaign with save and resume.
- A deterministic simulation core that can later be reused by an online server.

### Non-Goals (v1)

- Online multiplayer, accounts, matchmaking, leaderboards.
- Mouse input or clickable UI.
- Sound.
- Mobile or web builds.
- Pixel-level territory painting like the original. The world is a graph of regions.

## 3. Target Experience

A player launches the binary, sees an ASCII art splash, picks a preset, and lands on a colored world map. Within seconds they type `a fr` and watch a force march toward France. The map pulses with borders changing color, the leaderboard shifts, and the event log scrolls. The player is always one short command away from their next move, and the pressure to type quickly and accurately is the core fun.

## 4. Core Gameplay

### 4.1 World

- The map is a graph. Each node is a **region** (a country, or a sub-region for very large countries such as the US, Russia, China, Brazil, Canada, Australia and India so no single node dominates).
- Edges are **land borders** plus a curated set of **sea lanes** (for example UK to France, Japan to Korea, Indonesia chain) so islands are reachable.
- Each region has: `name`, `code` (short alias), `value` (economic weight, drives troop growth), `garrison` (defending troops), `owner`.
- Target region count: roughly 150 to 220 for the full world preset.
- Data source: Natural Earth (public domain), simplified and preprocessed at build time into a compact bitmap plus adjacency list, shipped inside the binary.

### 4.2 Economy and Troops

- Each player has a **troop pool** (unassigned troops) that grows over time.
- Growth rate scales with the total `value` of owned regions and slows as the pool approaches a cap that is also tied to territory. Growth is logistic: fast when low, slow near the cap. This keeps snowballing strong but not instant.
- Every region has a **garrison** that regenerates slowly up to a regional cap. Neutral regions have a base garrison so early expansion has a cost.
- All numbers live in a single tunable balance file.

### 4.3 Attacking

- You can only attack a region adjacent (by land border or sea lane) to one you own.
- `attack <target> [amount]` sends a force from your pool. Amount is a percent or a flat number. Without an amount, the configured default percent is used.
- Forces **march** along the graph edge. Travel time depends on edge length, so attacks are visible on the map and defenders can react.
- On arrival, combat resolves: if attackers exceed the garrison (multiplied by a defender bonus), the region is captured and the survivors become its new garrison. If not, both sides lose troops and the attack fails.
- Defender bonus applies per region (capitals and fortified regions get more). Exact values are tunable.

### 4.4 Defending and Support

- `fortify <region> <amount>` moves troops from the pool into a region's garrison.
- `recall <region> <amount>` moves troops from a garrison back to the pool (with a short delay).
- Combined attacks: several attack commands landing on the same target in a short window stack naturally.

### 4.5 Win Conditions

Configurable per match. Defaults:

- **Domination:** control a set percentage of total world value (default 60%).
- **Elimination:** last player standing.
- **Timed:** when the match timer ends, highest score wins (score = owned value plus a small troop bonus).

## 5. Command System (the twist)

The command line is the entire control scheme, so its UX is the most important product surface.

### 5.1 Design Principles

1. **Always on.** A persistent prompt at the bottom of the screen. Start typing and it captures input. No mode switching or `:` prefix needed. `Esc` clears the line.
2. **Short by default.** Verb, target, amount. Every verb has a one or two letter alias.
3. **Forgiving.** Fuzzy matching and aliases for region names, ghost-text autocomplete, `Tab` to accept.
4. **Fast repetition.** History with arrow keys, `!!` to repeat the last command, `;` to chain commands.
5. **Learnable.** `help` and inline hints show examples for whatever the player is typing.

### 5.2 Grammar

```
<verb> <target> [amount] [options]
```

| Verb | Alias | Example | Effect |
| --- | --- | --- | --- |
| attack | a | `a br 40%` | Send 40% of your pool toward Brazil |
| fortify | f | `f us-w 500` | Move 500 troops into a garrison |
| recall | r | `r fr 200` | Pull troops back to the pool |
| info | i | `i de` | Show region details and neighbors |
| focus | w | `w jp` | Move the camera to a region |
| status | s | `s` | Show your stats and rivals |
| speed | sp | `sp 2` | Change game speed (solo only) |
| pause | p | `p` | Pause (solo only) |
| alias |  | `alias rush a $1 80%` | Create a custom shortcut |
| help | h | `h attack` | Show help |

**Smart targets:** besides region codes and names, support relative targets that reduce typing under pressure:

- `@weakest` the weakest enemy region adjacent to you.
- `@richest` the highest value adjacent target.
- `@player:<name>` the nearest region owned by a given player.

Example: `a @weakest 30%`.

### 5.3 Autocomplete and Feedback

- Ghost text shows the most likely completion in dim color. `Tab` accepts it.
- A small hint strip above the prompt shows the parsed command live: `ATTACK  Brazil (garrison 120)  send 4,200 troops, ETA 8s`.
- Invalid commands fail visibly and instantly with a reason, and do not clear what you typed.
- Successful commands flash in the event log.

### 5.4 Non-command Keys

The game is command-driven, but a minimal set of view keys keeps the map usable: arrow keys to pan, `+` and `-` to zoom, `Tab` to cycle focus. These do not affect gameplay.

## 6. Bots

Bots play by the same rules as the player and issue the same commands through the same API. They are rate-limited so they cannot out-click a human with inhuman reaction speed.

### 6.1 Personalities

- **Aggressor:** high attack percentage, frequent wars, low defense.
- **Turtle:** consolidates and fortifies, attacks only with clear advantage.
- **Opportunist:** targets weakened neighbors and players engaged in other wars.
- **Warlord:** snowballs on one front at a time, then pivots.

### 6.2 Difficulty

Difficulty changes decision quality and reaction rate, not hidden resource bonuses.

| Level | Reaction interval | Behavior |
| --- | --- | --- |
| Easy | 6 to 10s | Simple expansion, poor defense |
| Normal | 3 to 5s | Evaluates odds before attacking |
| Hard | 1 to 2s | Coordinates attacks, exploits openings |

### 6.3 Bot Architecture

A utility-scoring decision loop: each tick, enumerate legal commands, score them by expected value (gain, risk, distance, personality weights), and issue the best one if the reaction timer allows. No machine learning needed for v1.

## 7. Visual and UI Design

### 7.1 Layout

```
+--------------------------------------------------------------+
| TERRITORIAL CLI    00:12:41   Pool 12,480 (+84/s)   Land 14% |
+------------------------------------------------+-------------+
|                                                | LEADERBOARD |
|                 WORLD MAP                      | 1 You   14% |
|            (half-block color render)           | 2 Bot-3 11% |
|                                                | 3 Bot-7  9% |
|                                                |  ...        |
+------------------------------------------------+-------------+
| EVENT LOG: Bot-3 captured Egypt | You took Spain       |
| > a fr 40%                                                  |
+--------------------------------------------------------------+
```

### 7.2 Map Rendering

- **Primary renderer:** half-block characters (`▀`) with truecolor foreground and background, giving two square-ish pixels per terminal cell. This is the best color and detail for the cost.
- **Region fill:** each region is rasterized once into a cell grid at startup. Ownership changes only recolor affected cells.
- **Borders:** slightly darker or lighter shade where adjacent cells belong to different regions.
- **Labels:** region codes and garrison counts appear as overlay text when zoom and terminal size allow. Troop movements render as small arrow glyphs along edges.
- **Zoom levels:** world overview, continent, and region detail.

### 7.2.1 Fallbacks

| Capability | Renderer |
| --- | --- |
| Truecolor + Unicode | Half-block (default) |
| 256 color | Palette-quantized half-block |
| No Unicode / old terminals | ASCII-only renderer using characters like `#`, `+`, `.` with 16 colors |
| Small terminal | Compact mode: map only, collapsed sidebar |

Minimum supported size: 100x30. Recommended: 160x50 or larger.

### 7.3 ASCII Art

- Splash screen and title banner.
- Match start countdown banner.
- Region capture and player elimination flourishes in the event log.
- Victory and defeat screens with a stats summary.
- Animated, but lightweight: a few frames, no heavy loops.

### 7.4 Color

- Player palette tuned for contrast and color-blind safety (provide at least one colorblind-friendly theme).
- Themes: dark default, high-contrast, colorblind-safe.

## 8. Technical Architecture

### 8.1 Why Rust + ratatui

- Single small static binary, no runtime to install.
- Very low CPU and memory use, which matters for a real-time render loop plus dozens of bots.
- ratatui diffs its buffer, so only changed cells are written to the terminal.
- crossterm gives cross-platform input and color, including Windows Terminal.

### 8.2 Crate Layout

```
territorial/
  core/      deterministic simulation, rules, map graph, bots (no I/O)
  render/    map rasterizer, themes, ASCII art assets
  ui/        ratatui layout, command line widget, input handling
  cli/       binary entrypoint, settings, save/load
  mapgen/    build-time tool: Natural Earth -> region bitmap + graph
```

### 8.3 Simulation Loop

- Fixed simulation tick (10 Hz), decoupled from rendering (target 30 to 60 FPS, redraw only on change).
- Input thread feeds a command queue. The sim consumes commands on tick boundaries.
- Simulation uses a seeded RNG and integer or fixed-point math so identical seeds and command logs replay identically.

### 8.4 Online-Readiness (designed now, built later)

- All player actions are timestamped **commands** applied to the sim. The local game already works this way.
- The `core` crate has no I/O, so it can run inside a server.
- Two candidate models for v2: authoritative server with snapshots plus deltas over WebSocket, or deterministic lockstep. Decision deferred, but determinism is a v1 requirement so replays and either model stay possible.

### 8.5 Performance Budget

| Metric | Target |
| --- | --- |
| Cold start | under 300 ms |
| Binary size | under 10 MB |
| Memory | under 50 MB with 40 bots |
| Idle CPU | under 3% of one core |
| Peak CPU | under 15% of one core during heavy action |
| Render | smooth at 30 FPS minimum on a 200x60 terminal |

## 9. Match Configuration

Configured from a setup screen or CLI flags.

| Setting | Options |
| --- | --- |
| Preset | Blitz (5 to 10 min), Standard (15 to 30 min), Epic (60 min+) |
| Custom length | Any timer; speed multiplier and growth scaling adjust to fit |
| Bots | 3 to 40 |
| Bot difficulty | Easy, Normal, Hard, Mixed |
| Starting region | Random or choose |
| Win condition | Domination %, Elimination, Timed |
| Map | World (v1); regional presets as stretch |
| Seed | Random or fixed |

Long matches support **save and resume** (serialize sim state plus seed).

## 10. Milestones

| Milestone | Scope | Exit criteria |
| --- | --- | --- |
| M0: Map pipeline | Natural Earth to regions, adjacency graph, sea lanes | Graph validates: connected, balanced region sizes |
| M1: Renderer | Half-block world map, camera, zoom, themes, fallbacks | 30 FPS on a 200x60 terminal |
| M2: Sim core | Economy, marching, combat, win conditions, tick loop | Deterministic replay test passes |
| M3: Command system | Parser, aliases, autocomplete, hints, smart targets | All verbs work with live feedback |
| M4: Bots | Personalities, difficulty, reaction limits | Bot-only matches complete without stalls |
| M5: Game flow | Setup screen, ASCII art, leaderboard, event log, save/load | Full match playable start to finish |
| M6: Balance and polish | Tuning, playtests, colorblind theme, perf pass | Budget targets met |
| M7 (post v1): Online | Server, protocol, lobbies | Separate PRD |

## 11. Success Metrics

- A new player completes a first match without reading external docs.
- Median command entry time under 3 seconds after the first two matches.
- Performance budget met on a mid-range laptop and Windows Terminal.
- Bot matches at each difficulty level end in a win condition (no endless stalemates) at least 95% of the time.
- Playtesters rate bots as "distinct" and "fair".

## 12. Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| Typing feels too slow for real-time pressure | Short aliases, smart targets, autocomplete, adjustable game speed, default attack percent |
| Map looks poor at low resolution | Region-based design (labels and borders matter more than pixels), zoom levels, ASCII overlays |
| Terminal inconsistencies, especially on Windows | Capability detection, renderer fallbacks, test matrix across terminals |
| Balance snowballing makes matches one-sided | Logistic growth caps, defender bonuses, bot rubber-banding via behavior (not cheats), playtesting |
| Region data quality (tiny countries, odd borders) | Merge micro-states into neighbors, curated sea lanes, split giant countries |
| Scope creep toward online too early | Keep sim I/O-free and deterministic, defer networking to a separate PRD |

## 13. Open Questions

1. Should region values be purely geographic (area and population based) or hand-tuned for gameplay balance?
2. Should the player's default attack percent be a global setting, or per-target-type?
3. Do we want alliances and diplomacy commands in v1, or only in the online phase?
4. How strict should the minimum terminal size be, versus auto-switching to compact mode?
5. Should replays be a v1 feature, given the sim is already deterministic?

## 14. Appendix: Example Session

```
> s
You: 1 region (Spain)  Pool 1,200  +18/s  Rank 7/12

> i pt
Portugal  garrison 90  value 3  neighbors: ES

> a pt 30%
ATTACK Portugal  send 360 troops  ETA 4s

> a @weakest 25% ; f es 200
ATTACK Andorra  send 300 | FORTIFY Spain +200
```
