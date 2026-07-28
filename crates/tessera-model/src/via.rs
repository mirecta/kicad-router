use serde::{Deserialize, Serialize};
use tessera_geom::Point;

use crate::layer::LayerId;
use crate::net::NetId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ViaId(pub u32);

/// A through-hole via spanning `from_layer` to `to_layer` inclusive.
///
/// Blind/buried/micro vias (plan §7.1) will need a richer span
/// representation than a simple two-layer range once stackup-legality
/// checks are in scope — left as a follow-up rather than modelled
/// speculatively now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Via {
    pub id: ViaId,
    pub position: Point,
    pub diameter_nm: i64,
    pub drill_nm: i64,
    pub from_layer: LayerId,
    pub to_layer: LayerId,
    pub net: NetId,
    pub locked: bool,
}
