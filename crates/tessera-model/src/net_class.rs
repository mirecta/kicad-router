use serde::{Deserialize, Serialize};

/// The clearance/width/via rules KiCad resolves per net class.
///
/// Field selection follows the real IPC `NetClassBoardSettings` schema
/// empirically verified in `docs/DECISIONS.md` ADR-0002 (`clearance`,
/// `track_width`, `diff_pair_track_width`, `diff_pair_gap`,
/// `diff_pair_via_gap` are first-class fields there). KiCad's real via rule
/// is a full `PadStack` (supports complex/blind/buried vias); this is
/// simplified here to flat through-hole diameter/drill, matching what
/// `tessera-drc`'s M1 clearance rules actually need — revisit once
/// blind/buried via legality (plan §7.1) is in scope.
///
/// This struct intentionally does **not** carry `diff_pair_uncoupled` — per
/// ADR-0002, that's a DRC *custom rule* constraint (`constraint
/// diff_pair_uncoupled (max ...)`), not a net-class property in KiCad's own
/// model, so it belongs in `tessera-drc`'s custom-rule engine, not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetClass {
    pub name: String,
    pub clearance_nm: i64,
    pub track_width_nm: i64,
    pub via_diameter_nm: i64,
    pub via_drill_nm: i64,
    pub diff_pair_track_width_nm: Option<i64>,
    pub diff_pair_gap_nm: Option<i64>,
    pub diff_pair_via_gap_nm: Option<i64>,
}

impl NetClass {
    /// KiCad's built-in fallback class every net belongs to unless
    /// otherwise assigned. Values here are placeholders — a real board
    /// always carries its own `"Default"` settings; this exists so tests
    /// and corpus fixtures have a sane class to reach for without repeating
    /// boilerplate.
    #[must_use]
    pub fn default_placeholder() -> Self {
        Self {
            name: "Default".to_string(),
            clearance_nm: 200_000,    // 0.2 mm
            track_width_nm: 250_000,  // 0.25 mm
            via_diameter_nm: 600_000, // 0.6 mm
            via_drill_nm: 300_000,    // 0.3 mm
            diff_pair_track_width_nm: None,
            diff_pair_gap_nm: None,
            diff_pair_via_gap_nm: None,
        }
    }
}
