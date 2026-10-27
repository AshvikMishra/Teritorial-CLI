//! Commands: the one API shared by the human player and the bots.
//!
//! Every gameplay action is a [`Command`] issued by a player. Commands are
//! validated by a single function (`Game::preview` uses it without side effects,
//! and `Game::step` uses it again when applying), so the UI hint strip, the bots
//! and the simulation always agree on what is legal.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{PlayerId, RegionId, Tick};

/// How many troops a command moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Amount {
    /// A percent (1..=100) of the source: the pool for attack and fortify,
    /// the region's movable garrison for recall. Resolved when applied.
    Percent(u8),
    /// A flat number of troops.
    Flat(u64),
}

/// A gameplay command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// Send troops from the pool to attack an adjacent region.
    Attack {
        /// Region to attack.
        target: RegionId,
        /// Troops to send.
        amount: Amount,
    },
    /// Move troops from the pool into an owned region's garrison (and fortify it).
    Fortify {
        /// Owned region.
        region: RegionId,
        /// Troops to move.
        amount: Amount,
    },
    /// Move troops from an owned region's garrison back to the pool (delayed).
    Recall {
        /// Owned region.
        region: RegionId,
        /// Troops to move.
        amount: Amount,
    },
    /// Propose an alliance, or accept a pending proposal from `with`.
    ProposeAlliance {
        /// The other player.
        with: PlayerId,
    },
    /// Break an alliance (starts a truce).
    BreakAlliance {
        /// The ally.
        with: PlayerId,
    },
}

impl Command {
    /// The long verb for this command, e.g. `attack`.
    pub fn verb(&self) -> &'static str {
        match self {
            Command::Attack { .. } => "attack",
            Command::Fortify { .. } => "fortify",
            Command::Recall { .. } => "recall",
            Command::ProposeAlliance { .. } => "ally",
            Command::BreakAlliance { .. } => "unally",
        }
    }

    /// The region this command acts on, if any.
    pub fn region(&self) -> Option<RegionId> {
        match self {
            Command::Attack { target, .. } => Some(*target),
            Command::Fortify { region, .. } => Some(*region),
            Command::Recall { region, .. } => Some(*region),
            Command::ProposeAlliance { .. } | Command::BreakAlliance { .. } => None,
        }
    }

    /// The other player this command addresses, if any.
    pub fn other_player(&self) -> Option<PlayerId> {
        match self {
            Command::ProposeAlliance { with } | Command::BreakAlliance { with } => Some(*with),
            _ => None,
        }
    }
}

/// A command stamped with the tick it was applied on and its issuer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedCommand {
    /// Tick the command was applied on.
    pub tick: Tick,
    /// Issuing player.
    pub player: PlayerId,
    /// The command.
    pub command: Command,
}

/// Why a command was rejected. `Display` gives a short human reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandError {
    /// The region id does not exist.
    UnknownRegion,
    /// The target is not adjacent to any region the issuer owns.
    NotAdjacent {
        /// Region name.
        region: String,
    },
    /// The issuer does not own the region.
    NotOwned {
        /// Region name.
        region: String,
    },
    /// The issuer already owns the attack target.
    OwnRegion {
        /// Region name.
        region: String,
    },
    /// The target's owner is an ally.
    Allied {
        /// Ally name.
        player: String,
    },
    /// A truce with the target's owner is in effect.
    Truce {
        /// Other player's name.
        player: String,
        /// Ticks until the truce ends.
        ticks_left: u64,
    },
    /// Not enough troops for the requested amount.
    InsufficientTroops {
        /// Troops required.
        needed: i64,
        /// Troops available.
        available: i64,
    },
    /// The amount is zero.
    ZeroAmount,
    /// A percent outside 1..=100.
    InvalidPercent {
        /// The percent given.
        pct: u8,
    },
    /// The player id does not exist.
    UnknownPlayer,
    /// The addressed player has been eliminated.
    PlayerEliminated {
        /// Player name.
        player: String,
    },
    /// The issuing player has been eliminated.
    IssuerEliminated,
    /// Alliances are disabled in this match.
    AlliancesDisabled,
    /// A player already has the maximum number of allies.
    AllianceLimit {
        /// Name of the player at the limit.
        player: String,
    },
    /// Already allied with that player.
    AlreadyAllied {
        /// Player name.
        player: String,
    },
    /// A proposal to that player is already pending.
    AlreadyProposed {
        /// Player name.
        player: String,
    },
    /// Not allied with that player.
    NotAllied {
        /// Player name.
        player: String,
    },
    /// A player command targeting oneself.
    SelfTarget,
    /// The match is over; no more commands are accepted.
    MatchOver,
    /// A smart target found no candidate.
    NoTarget,
    /// The player has no region bordering the issuer's territory.
    NoBorder {
        /// Player name.
        player: String,
    },
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::UnknownRegion => write!(f, "unknown region"),
            CommandError::NotAdjacent { region } => {
                write!(f, "{} is not adjacent to your territory", region)
            }
            CommandError::NotOwned { region } => write!(f, "you do not own {}", region),
            CommandError::OwnRegion { region } => write!(f, "{} is already yours", region),
            CommandError::Allied { player } => write!(f, "{} is your ally", player),
            CommandError::Truce { player, ticks_left } => {
                let t: u64 = *ticks_left;
                let secs = t.saturating_add(crate::TICKS_PER_SECOND - 1) / crate::TICKS_PER_SECOND;
                write!(f, "truce with {} ({}s left)", player, secs)
            }
            CommandError::InsufficientTroops { needed, available } => write!(
                f,
                "not enough troops (need {}, have {})",
                needed, available
            ),
            CommandError::ZeroAmount => write!(f, "amount must be greater than zero"),
            CommandError::InvalidPercent { pct } => {
                write!(f, "{}% is not a valid percentage (1-100)", pct)
            }
            CommandError::UnknownPlayer => write!(f, "unknown player"),
            CommandError::PlayerEliminated { player } => {
                write!(f, "{} has been eliminated", player)
            }
            CommandError::IssuerEliminated => write!(f, "you have been eliminated"),
            CommandError::AlliancesDisabled => write!(f, "alliances are disabled in this match"),
            CommandError::AllianceLimit { player } => {
                write!(f, "alliance limit reached ({})", player)
            }
            CommandError::AlreadyAllied { player } => write!(f, "already allied with {}", player),
            CommandError::AlreadyProposed { player } => {
                write!(f, "alliance already proposed to {}", player)
            }
            CommandError::NotAllied { player } => write!(f, "you are not allied with {}", player),
            CommandError::SelfTarget => write!(f, "you cannot target yourself"),
            CommandError::MatchOver => write!(f, "the match is over"),
            CommandError::NoTarget => write!(f, "no valid target"),
            CommandError::NoBorder { player } => {
                write!(f, "{} does not border your territory", player)
            }
        }
    }
}

impl std::error::Error for CommandError {}

/// What a command would do right now, from `Game::preview`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandPreview {
    /// Resolved amount that would be sent or moved (0 for alliance commands).
    pub troops: i64,
    /// Attack only: march time in ticks.
    pub eta_ticks: Option<u64>,
    /// Attack only: the owned region the march departs from.
    pub source: Option<RegionId>,
    /// Garrison of the target region (0 for alliance commands).
    pub target_garrison: i64,
    /// Owner of the target region (`None` = neutral or not a region command).
    pub target_owner: Option<PlayerId>,
    /// Attack: the region's current effective defense (`garrison * bonus / 1000`);
    /// an attack must exceed it to capture. 0 for other commands.
    pub defense_needed: i64,
}
