use serde::{Deserialize, Serialize};
use tessera_geom::{Circle, Point};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HoleId(pub u32);

/// A non-plated drilled hole (KiCad's `np_thru_hole` pad), e.g. a mounting
/// hole. It has no net and, when its pad size doesn't exceed its drill, no
/// copper at all — verified against real `kicad-cli`: only the board's
/// `min_hole_clearance` constrains copper near it, measured from the drill
/// edge; net-class clearance doesn't apply. It goes through every layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hole {
    pub id: HoleId,
    pub position: Point,
    pub diameter_nm: i64,
}

impl Hole {
    #[must_use]
    pub fn circle(&self) -> Circle {
        Circle::new(self.position, self.diameter_nm / 2)
    }
}
