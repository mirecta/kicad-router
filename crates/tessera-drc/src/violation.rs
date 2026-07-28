use serde::{Deserialize, Serialize};
use tessera_model::{PadId, TrackId, ViaId};

/// Identifies one side of a clearance violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ItemRef {
    Track(TrackId),
    Via(ViaId),
    Pad(PadId),
}

/// A single clearance rule violation between two board items.
///
/// Deliberately omits the actual measured distance for now — computing it
/// would mean converting an exact [`tessera_geom::Segment`]'s
/// `RationalDistanceSq` to a lossy `f64` purely for a report field, and
/// nothing in `tessera-drc` needs that yet. Add it once the M1 parity
/// harness (plan §3.2) needs to compare a reported distance against
/// KiCad's own DRC report, not before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearanceViolation {
    pub a: ItemRef,
    pub b: ItemRef,
    pub required_nm: i64,
}
