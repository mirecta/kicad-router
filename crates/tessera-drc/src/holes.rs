use serde::{Deserialize, Serialize};
use tessera_model::{Board, Hole, HoleId, Pad, ViaId};

use crate::clearance::{pad_clears_circle, verdict_threshold_nm, via_circle};
use crate::violation::ItemRef;

/// Copper too close to a non-plated hole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoleClearanceViolation {
    pub item: ItemRef,
    pub hole: HoleId,
    pub required_nm: i64,
}

/// Every track, via and pad closer to a non-plated hole than the board's
/// `min_hole_clearance` — measured from the drill edge, on every layer,
/// regardless of net (verified against real `kicad-cli`, including that
/// net-class clearance plays no part here and that the same 500nm
/// tolerance as copper clearance applies).
#[must_use]
pub fn check_hole_clearance(board: &Board) -> Vec<HoleClearanceViolation> {
    let required_nm = board.rules.min_hole_clearance_nm;
    let threshold_nm = verdict_threshold_nm(required_nm);
    let mut violations = Vec::new();
    for hole in &board.npth_holes {
        let circle = hole.circle();
        let mut report = |item| {
            violations.push(HoleClearanceViolation {
                item,
                hole: hole.id,
                required_nm,
            });
        };
        for t in &board.tracks {
            if !circle.clears_segment(t.segment, threshold_nm + t.width_nm / 2) {
                report(ItemRef::Track(t.id));
            }
        }
        for v in &board.vias {
            if !circle.clears_circle(via_circle(v), threshold_nm) {
                report(ItemRef::Via(v.id));
            }
        }
        for p in board.pads.iter().filter(|p| !is_own_copper(p, hole)) {
            if !pad_clears_circle(p, circle, threshold_nm) {
                report(ItemRef::Pad(p.id));
            }
        }
    }
    violations
}

/// A via whose annular ring is narrower than the board allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnularWidthViolation {
    pub via: ViaId,
    pub required_nm: i64,
}

/// Every via whose ring `(diameter - drill) / 2` is below the board's
/// `min_via_annular_width`. Verified against real `kicad-cli`: this check
/// is exact — unlike clearance there's no 500nm tolerance; a diameter
/// even 1nm too small is reported.
#[must_use]
pub fn check_via_annular_width(board: &Board) -> Vec<AnnularWidthViolation> {
    let required_nm = board.rules.min_via_annular_width_nm;
    board
        .vias
        .iter()
        .filter(|v| v.diameter_nm - v.drill_nm < 2 * required_nm)
        .map(|v| AnnularWidthViolation {
            via: v.id,
            required_nm,
        })
        .collect()
}

/// A non-plated pad whose copper is larger than its drill is modelled as
/// both a hole and a no-net pad at the same spot — that pad is the hole's
/// own copper, not something to keep clear of it.
fn is_own_copper(pad: &Pad, hole: &Hole) -> bool {
    pad.shape.center() == hole.position && pad.drill_nm == Some(hole.diameter_nm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::{Point, Segment};
    use tessera_model::{LayerId, NetId, Track, TrackId};

    fn board_with_hole_and_track(track_x_nm: i64) -> Board {
        let mut board = Board::new();
        board.npth_holes.push(Hole {
            id: HoleId(0),
            position: Point::new(0, 0),
            diameter_nm: 2_000_000,
        });
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(
                Point::new(track_x_nm, -2_000_000),
                Point::new(track_x_nm, 2_000_000),
            ),
            width_nm: 200_000,
            layer: LayerId(1),
            net: NetId(1),
            locked: false,
        });
        board
    }

    #[test]
    fn matches_kicads_threshold_and_tolerance() {
        // Hole edge at x=1_000_000, track half-width 100_000, default
        // min_hole_clearance 250_000: KiCad flags only > 500nm short.
        let edge_plus = |gap: i64| 1_000_000 + gap + 100_000;
        assert!(
            check_hole_clearance(&board_with_hole_and_track(edge_plus(250_000 - 500))).is_empty()
        );
        assert_eq!(
            check_hole_clearance(&board_with_hole_and_track(edge_plus(250_000 - 501))).len(),
            1
        );
    }

    #[test]
    fn annular_width_is_exact_with_no_tolerance() {
        let mut board = Board::new();
        board.rules.min_via_annular_width_nm = 200_000;
        for (id, diameter_nm) in [(0, 700_000), (1, 699_999)] {
            board.vias.push(tessera_model::Via {
                id: ViaId(id),
                position: Point::new(0, 0),
                diameter_nm,
                drill_nm: 300_000,
                from_layer: LayerId(0),
                to_layer: LayerId(1),
                net: NetId(1),
                locked: false,
            });
        }
        let violations = check_via_annular_width(&board);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].via, ViaId(1));
    }

    #[test]
    fn uses_the_boards_own_minimum() {
        let mut board = board_with_hole_and_track(1_000_000 + 300_000 + 100_000);
        assert!(check_hole_clearance(&board).is_empty());
        board.rules.min_hole_clearance_nm = 400_000;
        assert_eq!(check_hole_clearance(&board).len(), 1);
    }
}
