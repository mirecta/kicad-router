use serde::{Deserialize, Serialize};
use tessera_geom::Circle;

use crate::layer::LayerId;
use crate::net::NetId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PadId(pub u32);

/// A pad's copper shape, in absolute board coordinates.
///
/// Only `Circle` is modelled so far — `tessera-geom` doesn't have general
/// polygon/rectangle predicates yet (plan §4.2), and there is no clearance
/// check this crate could perform against a rect/rounded-rect/custom pad
/// today. Add variants here in lockstep with `tessera-geom` gaining the
/// matching exact predicate, not ahead of it — an unchecked shape variant
/// would be worse than an honest gap (plan §0's DRC-parity priority).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PadShape {
    Circle(Circle),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pad {
    pub id: PadId,
    pub shape: PadShape,
    pub layers: Vec<LayerId>,
    pub net: NetId,
    pub locked: bool,
}
