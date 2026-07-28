use serde::{Deserialize, Serialize};

/// Identifies a board layer. Stable across a single board's lifetime; not
/// meaningful across boards (KiCad reassigns layer ordinals per-board).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LayerId(pub u32);

/// Whether a layer carries copper (and is therefore routable/an obstacle
/// source) or is non-copper (silkscreen, mask, courtyard, ...).
///
/// Only `Copper` layers matter for routing itself; non-copper layers are
/// tracked because plan §3.1 requires silk/mask clearances to be modelled
/// even though tessera never *generates* silk/mask geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    Copper,
    NonCopper,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub kind: LayerKind,
}

impl Layer {
    #[must_use]
    pub fn copper(id: LayerId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            kind: LayerKind::Copper,
        }
    }
}
