//! Smart targets: relative targets such as `@weakest`, `@richest` and
//! `@player:<name>` that save typing under pressure.

use serde::{Deserialize, Serialize};

use crate::command::CommandError;
use crate::state::Game;
use crate::{PlayerId, RegionId};

/// A relative attack target, resolved against the current state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmartTarget {
    /// The attackable region with the lowest effective defense, preferring
    /// enemy-owned regions over neutral ones.
    Weakest,
    /// The attackable region with the highest value.
    Richest,
    /// The attackable region owned by this player with the shortest edge to
    /// your territory.
    Player(PlayerId),
}

/// Resolves a smart target for player `p` to a concrete region.
///
/// * `Weakest`: lowest `effective_defense` among enemy-owned attackable regions,
///   or among neutral ones when no enemy region borders you. Ties go to higher
///   value, then lower id.
/// * `Richest`: highest value. Ties go to lower defense, then lower id.
/// * `Player(x)`: region of `x` with the shortest edge to your territory. Ties go
///   to lower defense, then lower id. Errors if `x` does not border you.
pub fn resolve(game: &Game, p: PlayerId, t: &SmartTarget) -> Result<RegionId, CommandError> {
    if game.is_over() {
        return Err(CommandError::MatchOver);
    }
    match game.get_player(p) {
        Some(pl) => {
            if !pl.alive {
                return Err(CommandError::IssuerEliminated);
            }
        }
        None => return Err(CommandError::UnknownPlayer),
    }
    let candidates = game.attackable_regions(p);
    match *t {
        SmartTarget::Weakest => {
            let enemy: Vec<RegionId> = candidates
                .iter()
                .copied()
                .filter(|&r| game.region_state(r).owner.is_some())
                .collect();
            let pool: &[RegionId] = if enemy.is_empty() {
                candidates.as_slice()
            } else {
                enemy.as_slice()
            };
            // Key: (defense asc, value desc, id asc).
            let mut best: Option<(i64, u32, RegionId)> = None;
            for &r in pool {
                let def = game.effective_defense(r);
                let value = game.map().region(r).value;
                let better = match best {
                    None => true,
                    Some((bd, bv, _)) => def < bd || (def == bd && value > bv),
                };
                if better {
                    best = Some((def, value, r));
                }
            }
            best.map(|(_, _, r)| r).ok_or(CommandError::NoTarget)
        }
        SmartTarget::Richest => {
            // Key: (value desc, defense asc, id asc).
            let mut best: Option<(u32, i64, RegionId)> = None;
            for &r in &candidates {
                let def = game.effective_defense(r);
                let value = game.map().region(r).value;
                let better = match best {
                    None => true,
                    Some((bv, bd, _)) => value > bv || (value == bv && def < bd),
                };
                if better {
                    best = Some((value, def, r));
                }
            }
            best.map(|(_, _, r)| r).ok_or(CommandError::NoTarget)
        }
        SmartTarget::Player(x) => {
            if x == p {
                return Err(CommandError::SelfTarget);
            }
            let other = match game.get_player(x) {
                Some(pl) => pl,
                None => return Err(CommandError::UnknownPlayer),
            };
            if !other.alive {
                return Err(CommandError::PlayerEliminated {
                    player: other.name.clone(),
                });
            }
            if game.are_allied(p, x) {
                return Err(CommandError::Allied {
                    player: other.name.clone(),
                });
            }
            if let Some(left) = game.truce_ticks_left(p, x) {
                return Err(CommandError::Truce {
                    player: other.name.clone(),
                    ticks_left: left,
                });
            }
            // Key: (edge length asc, defense asc, id asc).
            let mut best: Option<(u32, i64, RegionId)> = None;
            for &r in &candidates {
                if game.region_state(r).owner != Some(x) {
                    continue;
                }
                let mut shortest: Option<u32> = None;
                for &(nb, len, _) in game.map().neighbors(r) {
                    if game.region_state(nb).owner == Some(p) {
                        shortest = Some(match shortest {
                            None => len,
                            Some(s) => s.min(len),
                        });
                    }
                }
                let len = match shortest {
                    Some(l) => l,
                    None => continue,
                };
                let def = game.effective_defense(r);
                let better = match best {
                    None => true,
                    Some((bl, bd, _)) => len < bl || (len == bl && def < bd),
                };
                if better {
                    best = Some((len, def, r));
                }
            }
            match best {
                Some((_, _, r)) => Ok(r),
                None => Err(CommandError::NoBorder {
                    player: other.name.clone(),
                }),
            }
        }
    }
}
