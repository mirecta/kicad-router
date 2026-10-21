use serde::{Deserialize, Serialize};
use tessera_geom::Polygon;

use crate::layer::LayerId;
use crate::net::NetId;

/// One filled polygon of a copper zone (pour), as stored in the board file.
/// Verified against real `kicad-cli`: DRC checks other copper against this
/// stored *fill* — an unfilled zone's outline constrains nothing — and the
/// required distance is the larger of the zone's own clearance and the
/// net-class clearance (see [`crate::Board::zone_clearance_nm`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoneFill {
    pub net: NetId,
    pub layer: LayerId,
    /// The zone's own `(connect_pads (clearance ..))`, `0` if absent.
    pub clearance_nm: i64,
    pub polygon: Polygon,
    /// How far KiCad grows the stored fill on load: half the zone's
    /// `min_thickness` for files older than format `20250210` (the legacy
    /// "outline stroked at min thickness" fill representation), `0` from
    /// then on — both verified against real `kicad-cli` by bisecting the
    /// file version. Growing a region by `r` lowers every distance to it
    /// by exactly `r`, so it simply adds to the required clearance.
    pub inflation_nm: i64,
}

/// A copper zone that exists in the board file, filled or not — just which
/// net it belongs to and which layers it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopperZone {
    pub net: NetId,
    pub layers: Vec<LayerId>,
}

/// A copper layer reserved as a plane for one net (e.g. In1.Cu = +3.3V,
/// In2.Cu = GND on a 4-layer board). No signal is routed on it; the net's
/// pads reach it through their own hole or a short fanout via instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plane {
    pub layer: LayerId,
    pub net: NetId,
}
