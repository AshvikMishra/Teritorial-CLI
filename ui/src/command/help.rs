//! Help text: the command overview, per-verb help with examples (PRD 5.2) and
//! topic pages for smart targets, amounts, chaining and view keys.
//!
//! Every function returns plain lines (no styling) so the app can render them
//! in a popup or the event log.

use super::parser::Verb;

/// Help topics that are not verbs.
pub const TOPICS: [&str; 4] = ["targets", "amounts", "chain", "keys"];

struct VerbDoc {
    usage: &'static str,
    summary: &'static str,
    details: &'static [&'static str],
    examples: &'static [(&'static str, &'static str)],
}

fn doc(v: Verb) -> VerbDoc {
    match v {
        Verb::Attack => VerbDoc {
            usage: "attack <target> [amount]",
            summary: "send troops from your pool to an adjacent region",
            details: &[
                "The target must border your territory by land or by sea lane.",
                "Troops leave your pool now and march; the ETA depends on the edge length.",
                "They capture the region if they exceed its garrison times its defense bonus.",
                "Without an amount you send your default percent (one for neutral, one for enemy regions).",
                "Targets: code (fr), name (france), prefix (fra) or a smart target (help targets).",
            ],
            examples: &[
                ("a br 40%", "send 40% of your pool toward the Brazil region you border"),
                ("a fr", "attack France with your default percent"),
                ("a @weakest 30%", "hit the weakest enemy region next to you"),
                ("a @p:bot-3 half", "half your pool at Bot-3's nearest region"),
            ],
        },
        Verb::Fortify => VerbDoc {
            usage: "fortify <region> <amount>",
            summary: "move pool troops into one of your garrisons",
            details: &[
                "The region must be yours. The amount is required.",
                "Fortified regions get a temporary defense bonus.",
            ],
            examples: &[
                ("f us-w 500", "move 500 troops into the western US"),
                ("f es 25%", "move a quarter of your pool into Spain"),
            ],
        },
        Verb::Recall => VerbDoc {
            usage: "recall <region> <amount>",
            summary: "pull garrison troops back to the pool",
            details: &[
                "The region must be yours and keeps at least 1 troop. The amount is required.",
                "A percent is a percent of the movable garrison. Troops arrive after a short delay.",
            ],
            examples: &[
                ("r fr 200", "pull 200 troops out of France"),
                ("r fr all", "pull everything but 1 troop"),
            ],
        },
        Verb::Ally => VerbDoc {
            usage: "ally <player>",
            summary: "propose an alliance, or accept one",
            details: &[
                "Allies cannot attack each other. Proposals expire after a while.",
                "If the other player already proposed to you, this accepts.",
                "Players: name (bot-3), unique prefix, or number (3).",
            ],
            examples: &[
                ("al bot-3", "propose an alliance to Bot-3, or accept theirs"),
                ("al 3", "the same, by player number"),
            ],
        },
        Verb::Unally => VerbDoc {
            usage: "unally <player>",
            summary: "break an alliance (a truce follows)",
            details: &["During the truce neither side may attack the other."],
            examples: &[("ua bot-3", "break the alliance with Bot-3")],
        },
        Verb::Info => VerbDoc {
            usage: "info [region]",
            summary: "region details and neighbors",
            details: &["Without a region it shows your capital."],
            examples: &[
                ("i de", "Germany's owner, garrison, value and neighbors"),
                ("i", "your capital"),
            ],
        },
        Verb::Focus => VerbDoc {
            usage: "focus [region]",
            summary: "move the camera to a region",
            details: &["Without a region it returns to your capital."],
            examples: &[
                ("w jp", "center the camera on Japan"),
                ("w @weakest", "look at your weakest neighbor"),
                ("w", "back to your capital"),
            ],
        },
        Verb::Status => VerbDoc {
            usage: "status",
            summary: "your stats and rivals",
            details: &["Regions, pool, growth, land share and rank."],
            examples: &[("s", "show your status")],
        },
        Verb::Speed => VerbDoc {
            usage: "speed [multiplier]",
            summary: "change game speed (solo only)",
            details: &[
                "From 0.25x to 8x. Also accepts 2x or 150%.",
                "Without a value it returns to normal speed.",
            ],
            examples: &[
                ("sp 2", "double speed"),
                ("sp 0.5", "half speed"),
                ("sp", "normal speed"),
            ],
        },
        Verb::Pause => VerbDoc {
            usage: "pause",
            summary: "pause or resume (solo only)",
            details: &["Type it again to resume."],
            examples: &[("p", "pause the game")],
        },
        Verb::Alias => VerbDoc {
            usage: "alias [name [command...]]",
            summary: "create a custom shortcut",
            details: &[
                "$1..$9 become the arguments, $* all of them; without placeholders arguments are appended.",
                "Quote a body that chains with ;  Names cannot reuse built-in verbs or their short forms.",
                "Without arguments it lists your aliases.",
            ],
            examples: &[
                ("alias rush a $1 80%", "then 'rush fr' sends 80% at France"),
                ("alias go \"a @weakest 30%; f es 10%\"", "a chained shortcut"),
                ("alias", "list your aliases"),
            ],
        },
        Verb::Unalias => VerbDoc {
            usage: "unalias <name>",
            summary: "remove a custom shortcut",
            details: &[],
            examples: &[("unalias rush", "remove the rush alias")],
        },
        Verb::Help => VerbDoc {
            usage: "help [command|topic]",
            summary: "show help",
            details: &["Topics: targets, amounts, chain, keys."],
            examples: &[
                ("h attack", "help for attack"),
                ("h targets", "smart targets and region names"),
                ("h keys", "view keys"),
            ],
        },
        Verb::Save => VerbDoc {
            usage: "save [name]",
            summary: "save the match",
            details: &["Names use letters, digits, - and _. Without a name it quick-saves."],
            examples: &[
                ("sv", "quick save"),
                ("sv epic1", "save as epic1"),
            ],
        },
        Verb::Quit => VerbDoc {
            usage: "quit",
            summary: "leave the match",
            details: &[],
            examples: &[("q", "quit")],
        },
        Verb::Theme => VerbDoc {
            usage: "theme [dark|high-contrast|colorblind]",
            summary: "switch the color theme",
            details: &["Without a name it cycles to the next theme. Short forms: hc, cb."],
            examples: &[
                ("theme hc", "high-contrast colors"),
                ("theme cb", "colorblind-safe colors"),
                ("theme", "next theme"),
            ],
        },
    }
}

/// The usage line of a verb, e.g. `attack <target> [amount]`.
pub fn usage(v: Verb) -> &'static str {
    doc(v).usage
}

/// A one-line description of a verb.
pub fn summary(v: Verb) -> &'static str {
    doc(v).summary
}

/// The primary example of a verb, e.g. `a br 40%`.
pub fn example(v: Verb) -> &'static str {
    let d = doc(v);
    match d.examples.first() {
        Some(e) => e.0,
        None => d.usage,
    }
}

/// Every word `help` accepts directly: verb names, then topics.
pub fn topic_words() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Verb::ALL.iter().map(|v| v.name()).collect();
    for t in TOPICS {
        out.push(t);
    }
    out
}

/// Maps a help argument to its canonical topic: a verb name (`a` -> `attack`)
/// or one of [`TOPICS`] (with a few synonyms, and `@...` -> `targets`).
pub fn topic_key(word: &str) -> Option<&'static str> {
    let trimmed = word.trim();
    if trimmed.starts_with('@') {
        return Some("targets");
    }
    let w = trimmed.to_lowercase();
    if let Some(v) = Verb::from_exact(&w) {
        return Some(v.name());
    }
    for t in TOPICS {
        if t == w.as_str() {
            return Some(t);
        }
    }
    if let Some(v) = Verb::lookup(&w) {
        return Some(v.name());
    }
    if w.chars().count() >= 2 {
        for t in TOPICS {
            if t.starts_with(w.as_str()) {
                return Some(t);
            }
        }
    }
    match w.as_str() {
        "smart" | "target" | "region" | "regions" | "names" => Some("targets"),
        "amount" | "numbers" | "number" | "percent" => Some("amounts"),
        "chaining" | "history" | "repeat" | "!!" | ";" | "aliases" => Some("chain"),
        "key" | "keyboard" | "view" | "camera" | "zoom" | "pan" => Some("keys"),
        _ => None,
    }
}

/// The command overview shown by plain `help`.
pub fn overview() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    out.push("COMMANDS  <verb> <target> [amount]    chain with ;    repeat with !!".to_string());
    for v in Verb::ALL.iter().copied() {
        let d = doc(v);
        out.push(format!(
            "  {:<8} {:<3} {:<20} {}",
            v.name(),
            v.short().unwrap_or(""),
            example(v),
            d.summary
        ));
    }
    out.push("SMART TARGETS  @weakest  @richest  @player:<name> (or @p:<name>)".to_string());
    out.push("AMOUNTS  40%  500  4,200  1.2k  half  all".to_string());
    out.push(
        "KEYS  Tab accept hint | Up/Down history | Esc clear | Shift/Ctrl+arrows pan | +/- zoom (empty prompt) | PgUp/PgDn log"
            .to_string(),
    );
    out.push("MORE  help <command>    help targets | amounts | chain | keys".to_string());
    out
}

/// Full help for one verb: title, usage, details and examples.
pub fn verb_help(v: Verb) -> Vec<String> {
    let d = doc(v);
    let mut out: Vec<String> = Vec::new();
    match v.short() {
        Some(s) => out.push(format!("{} ({})  {}", v.name().to_uppercase(), s, d.summary)),
        None => out.push(format!("{}  {}", v.name().to_uppercase(), d.summary)),
    }
    out.push(format!("usage: {}", d.usage));
    for line in d.details {
        out.push(format!("  {}", line));
    }
    out.push("examples:".to_string());
    for (cmd, what) in d.examples {
        out.push(format!("  {:<24} {}", cmd, what));
    }
    out
}

const TARGETS_PAGE: &[&str] = &[
    "TARGETS  how to name regions and players",
    "  Regions: code (fr, us-w), name (france, south-africa), a unique prefix (fra),",
    "  a word or initials (zealand, nz), or a near miss (frnace). Case does not matter.",
    "  If several regions match, the one that fits the command wins (attackable for",
    "  attack, yours for fortify/recall); otherwise you get a list of candidates.",
    "  Multi-word names can be typed with spaces: a south africa 30%",
    "Smart targets (attack, info, focus):",
    "  @weakest        the weakest enemy region next to you (neutral if no enemy borders you)",
    "  @richest        the highest-value region you can attack",
    "  @player:<name>  the nearest region of that player (short: @p:<name>)",
    "  Short forms: @w, @r.  Example: a @weakest 30%",
    "Players: name (bot-3), unique prefix (bot-1 vs bot-12 needs the full name), or number (3).",
];

const AMOUNTS_PAGE: &[&str] = &[
    "AMOUNTS",
    "  40%    a percent of your pool (of the movable garrison for recall)",
    "  500    a flat number of troops; 4,200 and 4_200 work too",
    "  1.2k   thousands (2k = 2,000); 1.5m = 1,500,000",
    "  half   50%        all / max   100%",
    "  attack without an amount uses your default percent (neutral and enemy defaults in settings).",
    "  fortify and recall always need an amount.",
    "  The amount may come before the target: a 40% fr",
];

const CHAIN_PAGE: &[&str] = &[
    "CHAINING, REPEAT, HISTORY AND ALIASES",
    "  a @weakest 25% ; f es 200   run several commands at once with ;",
    "  !!                          repeat the last command line",
    "  !! 60%                      repeat it with extra text appended",
    "  Up / Down                   browse history; your unfinished line comes back",
    "  Esc                         clear the line",
    "  alias rush a $1 80%         define a shortcut: rush fr = a fr 80%",
    "  Invalid commands are not run and stay on the line so you can fix them.",
];

const KEYS_PAGE: &[&str] = &[
    "VIEW KEYS (these never affect gameplay)",
    "  Shift/Ctrl + arrows   pan the map",
    "  + and -               zoom in and out (only when the prompt is empty)",
    "  Tab                   accept the dimmed completion; with an empty prompt, cycle the camera through your regions",
    "  Up / Down             command history",
    "  Left/Right/Home/End   move the cursor; Backspace/Delete edit",
    "  PgUp / PgDn           scroll the event log",
    "  Esc                   clear the command line",
];

/// The page for a non-verb topic from [`TOPICS`], or `None` if unknown.
pub fn topic_lines(topic: &str) -> Option<Vec<String>> {
    let lines: &[&str] = match topic {
        "targets" => TARGETS_PAGE,
        "amounts" => AMOUNTS_PAGE,
        "chain" => CHAIN_PAGE,
        "keys" => KEYS_PAGE,
        _ => return None,
    };
    Some(lines.iter().map(|s| s.to_string()).collect())
}

/// Help for an optional topic: `None` gives the overview; a verb, short form
/// or topic gives that page; anything else gives a notice plus the overview.
pub fn help_lines(topic: Option<&str>) -> Vec<String> {
    let raw = match topic {
        Some(t) if !t.trim().is_empty() => t,
        _ => return overview(),
    };
    match topic_key(raw) {
        Some(key) => {
            if let Some(v) = Verb::from_exact(key) {
                return verb_help(v);
            }
            match topic_lines(key) {
                Some(lines) => lines,
                None => overview(),
            }
        }
        None => {
            let mut out = vec![format!("No help for '{}'.", raw.trim())];
            out.extend(overview());
            out
        }
    }
}
