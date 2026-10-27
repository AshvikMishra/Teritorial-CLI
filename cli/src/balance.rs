//! Balance loading: the embedded `assets/balance.toml`, optionally overridden
//! key by key from a `--balance` file.

use std::fs;
use std::path::Path;

use serde::Deserialize;
use territorial_core::Balance;

use crate::text::edit_distance;

/// The shipped balance file, embedded at compile time.
const EMBEDDED_BALANCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../assets/balance.toml"
));

/// Loads the balance. With `override_file`, every key in that file replaces the
/// embedded value; keys it does not mention keep their embedded values. Unknown
/// keys are an error so that typos do not go unnoticed.
pub fn load(override_file: Option<&Path>) -> Result<Balance, String> {
    let mut table: toml::Table = EMBEDDED_BALANCE
        .parse::<toml::Table>()
        .map_err(|e| format!("the built-in balance file is invalid: {}", e))?;

    let override_text: Option<(String, String)> = match override_file {
        Some(path) => {
            let shown = path.display().to_string();
            let text = fs::read_to_string(path)
                .map_err(|e| format!("cannot read balance file {}: {}", shown, e))?;
            Some((shown, text))
        }
        None => None,
    };

    if let Some((shown, text)) = &override_text {
        let over: toml::Table = text
            .parse::<toml::Table>()
            .map_err(|e| format!("balance file {} is not valid TOML:\n{}", shown, e))?;
        let mut unknown: Vec<String> = Vec::new();
        for (key, value) in over {
            if table.contains_key(key.as_str()) {
                table.insert(key, value);
            } else {
                unknown.push(describe_unknown(&table, &key));
            }
        }
        if !unknown.is_empty() {
            return Err(format!(
                "balance file {} has unknown key(s): {}. Valid keys are listed in assets/balance.toml.",
                shown,
                unknown.join(", ")
            ));
        }
    }

    let balance: Balance = match Balance::deserialize(table) {
        Ok(b) => b,
        Err(e) => {
            // The merged table has no source positions; re-parse the override
            // file on its own to report the offending line when possible.
            if let Some((shown, text)) = &override_text {
                if let Err(e2) = toml::from_str::<Balance>(text) {
                    return Err(format!("balance file {} is invalid:\n{}", shown, e2));
                }
                return Err(format!("balance file {} is invalid: {}", shown, e));
            }
            return Err(format!("the built-in balance file is invalid: {}", e));
        }
    };

    balance.validate().map_err(|e| match &override_text {
        Some((shown, _)) => format!("balance file {} has invalid values: {}", shown, e),
        None => format!("the built-in balance values are invalid: {}", e),
    })?;
    Ok(balance)
}

/// `key`, plus the closest valid key if one is near.
fn describe_unknown(valid: &toml::Table, key: &str) -> String {
    let mut best: Option<(usize, &str)> = None;
    for candidate in valid.keys() {
        let d = edit_distance(candidate, key);
        let better = match best {
            None => true,
            Some((bd, _)) => d < bd,
        };
        if better {
            best = Some((d, candidate.as_str()));
        }
    }
    match best {
        Some((d, candidate)) if d <= 3 => format!("'{}' (did you mean '{}'?)", key, candidate),
        _ => format!("'{}'", key),
    }
}
