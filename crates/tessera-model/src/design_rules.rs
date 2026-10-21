use serde::{Deserialize, Serialize};

/// Board-wide manufacturing minimums (KiCad's Board Setup → Constraints,
/// stored in `.kicad_pro` under `board.design_settings.rules`), separate
/// from per-net-class clearance/width. Only the ones something in this
/// workspace checks or routes against are modelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignRules {
    /// Minimum distance from a hole's edge to copper of another item.
    pub min_hole_clearance_nm: i64,
    /// Minimum edge-to-edge distance between two drilled holes, regardless
    /// of net (verified: two vias of the same net are flagged too).
    pub min_hole_to_hole_nm: i64,
    /// Minimum via annular ring, `(diameter - drill) / 2`.
    pub min_via_annular_width_nm: i64,
    /// Minimum distance from copper to the board edge (`Edge.Cuts` lines),
    /// regardless of net — verified against real `kicad-cli`.
    pub min_copper_edge_clearance_nm: i64,
}

impl Default for DesignRules {
    /// KiCad 10's own defaults, used when a project doesn't set a value —
    /// read from the limits real `kicad-cli` reports for a board with no
    /// `.kicad_pro` at all.
    fn default() -> Self {
        Self {
            min_hole_clearance_nm: 250_000,
            min_hole_to_hole_nm: 250_000,
            min_via_annular_width_nm: 100_000,
            min_copper_edge_clearance_nm: 500_000,
        }
    }
}
