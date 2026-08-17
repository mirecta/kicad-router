use serde::{Deserialize, Serialize};
use tessera_geom::{Circle, Oval, Rect};

use crate::layer::LayerId;
use crate::net::NetId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PadId(pub u32);

/// A pad's copper shape, in absolute board coordinates.
///
/// `Circle`, axis-aligned `Rect`, and `Oval` are modelled (2026-08-17:
/// `Rect` and `Oval` added once `tessera-geom` gained exact clearance
/// predicates for each — see those types' own docs for the still-real
/// limitations: no footprint rotation beyond 90° multiples,
/// `roundrect`'s corner rounding not modelled as its own shape).
/// `Polygon`/custom pad shapes still have no clearance check this crate
/// could perform, so still aren't a variant here — add one in lockstep
/// with `tessera-geom` gaining the matching exact predicate, not ahead
/// of it — an unchecked shape variant would be worse than an honest gap
/// (plan §0's DRC-parity priority).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PadShape {
    Circle(Circle),
    Rect(Rect),
    Oval(Oval),
}

impl PadShape {
    /// This shape's own centre point — where a router lands a track on
    /// this pad, and the connection endpoint `Board::
    /// find_unrouted_connections` reports, regardless of which concrete
    /// shape this is.
    #[must_use]
    pub const fn center(&self) -> tessera_geom::Point {
        match self {
            Self::Circle(c) => c.center,
            Self::Rect(r) => r.center,
            Self::Oval(o) => o.center,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pad {
    pub id: PadId,
    pub shape: PadShape,
    pub layers: Vec<LayerId>,
    pub net: NetId,
    pub locked: bool,
    /// The owning footprint's reference designator (e.g. `"IC14"`), if
    /// known — needed to evaluate `.kicad_dru`'s `fromTo('IC14-*', ...)`
    /// custom-rule predicate. Matching against it is against
    /// `"reference-padnumber"` (see [`Pad::number`]) or bare
    /// `"reference"` alone, case-insensitively — see
    /// `docs/DECISIONS.md`'s "Wildcard/pairwise-binding semantics" entry
    /// for the empirical grounding. `None` for pads with no known owning
    /// footprint (e.g. synthetic test boards that don't model
    /// footprints, or any future pad source that isn't `.kicad_pcb`).
    pub reference: Option<String>,
    /// This pad's own KiCad-native designator (e.g. `"1"`, `"A3"`,
    /// `"GND"` — not necessarily numeric), the part after the reference
    /// in `fromTo`'s `"reference-padnumber"` match target. `None`
    /// alongside `reference: None` for the same reason.
    pub number: Option<String>,
    /// This pad's own drilled-hole diameter, if it has one — `Some(d)` for
    /// a through-hole pad (KiCad's `thru_hole`/`np_thru_hole` pad types),
    /// `None` for a surface-mount pad (`smd`), needed to evaluate
    /// `.kicad_dru`'s `hole_size` constraint against real pad holes, not
    /// just via holes (see `docs/DECISIONS.md`'s `hole_size`/`via_diameter`
    /// entry — verified against real `kicad-cli` that `hole_size` fires
    /// for PTH pads too, not only vias). Circular drills only, matching
    /// this crate's existing "only `Circle` pad shape modelled" scope —
    /// KiCad's oval/slotted drills aren't represented here.
    pub drill_nm: Option<i64>,
}
