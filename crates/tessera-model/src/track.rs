use serde::{Deserialize, Serialize};
use tessera_geom::Segment;

use crate::layer::LayerId;
use crate::net::NetId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TrackId(pub u32);

/// A straight copper track segment. Arc tracks are a deliberate omission —
/// see `tessera_geom::Segment`'s doc comment; they'll need their own
/// variant once `tessera-geom` models arcs natively (plan §4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub segment: Segment,
    pub width_nm: i64,
    pub layer: LayerId,
    pub net: NetId,
    pub locked: bool,
}
