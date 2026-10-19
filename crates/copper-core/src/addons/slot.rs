//! Slots: the named regions of the window an addon can take over
//! (contract §1). The list is closed and compiled in; the cloud refuses
//! uploads for slots it does not know, and a client ignores a resolved addon
//! for a slot it does not know.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A slot this build knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SlotId {
    /// The right-hand pane toggled by ⌘E / Ctrl+E.
    #[serde(rename = "agent-sidebar")]
    AgentSidebar,
}

impl SlotId {
    /// Every slot, in display order.
    pub const ALL: &'static [SlotId] = &[SlotId::AgentSidebar];

    /// The wire id (`"agent-sidebar"`).
    pub fn as_str(self) -> &'static str {
        match self {
            SlotId::AgentSidebar => "agent-sidebar",
        }
    }

    /// For people (`"Agent sidebar"`).
    pub fn name(self) -> &'static str {
        match self {
            SlotId::AgentSidebar => "Agent sidebar",
        }
    }

    /// One line on what the slot is.
    pub fn description(self) -> &'static str {
        match self {
            SlotId::AgentSidebar => "The pane beside the page, toggled with Ctrl+E (⌘E on a Mac).",
        }
    }

    /// The slot id grammar, `^[a-z][a-z0-9-]{1,40}$`, whether or not this
    /// build knows the slot.
    pub fn is_well_formed(text: &str) -> bool {
        let b = text.as_bytes();
        (2..=41).contains(&b.len())
            && b[0].is_ascii_lowercase()
            && b[1..]
                .iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
    }
}

impl fmt::Display for SlotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A slot id this build does not know.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown slot {0:?}")]
pub struct UnknownSlot(pub String);

impl FromStr for SlotId {
    type Err = UnknownSlot;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SlotId::ALL
            .iter()
            .copied()
            .find(|slot| slot.as_str() == s)
            .ok_or_else(|| UnknownSlot(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!("agent-sidebar".parse::<SlotId>(), Ok(SlotId::AgentSidebar));
        assert!("left-rail".parse::<SlotId>().is_err());
        assert_eq!(
            serde_json::to_string(&SlotId::AgentSidebar).unwrap(),
            "\"agent-sidebar\""
        );
        assert_eq!(SlotId::AgentSidebar.name(), "Agent sidebar");
        for ok in ["agent-sidebar", "ab", "a1-"] {
            assert!(SlotId::is_well_formed(ok), "{ok}");
        }
        for bad in ["a", "1a", "Agent", "a_b", "", &"a".repeat(42)] {
            assert!(!SlotId::is_well_formed(bad), "{bad}");
        }
    }
}
