use serde::{Deserialize, Serialize};
use tessera_geom::Segment;
use tessera_model::{Board, LayerId, Pad, PadShape, RuleAreaId, Via};

use crate::clearance::{verdict_threshold_nm, via_circle};
use crate::violation::ItemRef;

/// Copper closer to the board edge than `min_copper_edge_clearance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeClearanceViolation {
    pub item: ItemRef,
    pub required_nm: i64,
}

/// A track or via overlapping a keepout area that forbids it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeepoutViolation {
    pub item: ItemRef,
    pub area: RuleAreaId,
}

fn edges(board: &Board) -> Vec<Segment> {
    board.outline.as_ref().map_or_else(Vec::new, |outline| {
        let n = outline.vertices.len();
        (0..n)
            .map(|i| Segment::new(outline.vertices[i], outline.vertices[(i + 1) % n]))
            .collect()
    })
}

fn pad_clears_segment(pad: &Pad, edge: Segment, min_nm: i64) -> bool {
    match &pad.shape {
        PadShape::Circle(c) => c.clears_segment(edge, min_nm),
        PadShape::Rect(r) => r.clears_segment(edge, min_nm),
        PadShape::Oval(o) => o.clears_segment(edge, min_nm),
    }
}

/// Every track, via and pad closer to a board-outline edge than the
/// board's `min_copper_edge_clearance`. Verified against real `kicad-cli`:
/// measured to the edge line (crossing it measures 0), regardless of net,
/// with the same 500nm tolerance as clearance; copper wholly outside the
/// board but far from the line isn't reported.
#[must_use]
pub fn check_edge_clearance(board: &Board) -> Vec<EdgeClearanceViolation> {
    let required_nm = board.rules.min_copper_edge_clearance_nm;
    let min_nm = verdict_threshold_nm(required_nm);
    let edges = edges(board);
    let mut violations = Vec::new();
    let mut check = |item, clears: &dyn Fn(Segment) -> bool| {
        if !edges.iter().all(|&e| clears(e)) {
            violations.push(EdgeClearanceViolation { item, required_nm });
        }
    };
    for t in &board.tracks {
        check(ItemRef::Track(t.id), &|e| {
            e.clears_segment(t.segment, min_nm + t.width_nm / 2)
        });
    }
    for v in &board.vias {
        check(ItemRef::Via(v.id), &|e| {
            via_circle(v).clears_segment(e, min_nm)
        });
    }
    for p in &board.pads {
        check(ItemRef::Pad(p.id), &|e| pad_clears_segment(p, e, min_nm));
    }
    violations
}

fn via_spans(via: &Via, layer: LayerId) -> bool {
    let lo = via.from_layer.0.min(via.to_layer.0);
    let hi = via.from_layer.0.max(via.to_layer.0);
    (lo..=hi).contains(&layer.0)
}

/// Every track or via overlapping a keepout rule area, on one of its
/// layers, that forbids that kind of item. Verified against real
/// `kicad-cli` (`items_not_allowed`): any overlap of the item's copper
/// counts, being merely near the area doesn't.
#[must_use]
pub fn check_keepouts(board: &Board) -> Vec<KeepoutViolation> {
    let mut violations = Vec::new();
    for area in &board.rule_areas {
        if !area.keepout.tracks_allowed {
            for t in board
                .tracks
                .iter()
                .filter(|t| area.layers.contains(&t.layer))
            {
                if !area.outline.clears_segment(t.segment, t.width_nm / 2) {
                    violations.push(KeepoutViolation {
                        item: ItemRef::Track(t.id),
                        area: area.id,
                    });
                }
            }
        }
        if !area.keepout.vias_allowed {
            for v in board
                .vias
                .iter()
                .filter(|v| area.layers.iter().any(|&l| via_spans(v, l)))
            {
                if !area.outline.clears_circle(via_circle(v), 0) {
                    violations.push(KeepoutViolation {
                        item: ItemRef::Via(v.id),
                        area: area.id,
                    });
                }
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::{Point, Polygon};
    use tessera_model::{KeepoutFlags, NetId, RuleArea, Track, TrackId};

    fn square(lo: i64, hi: i64) -> Polygon {
        Polygon::new(vec![
            Point::new(lo, lo),
            Point::new(hi, lo),
            Point::new(hi, hi),
            Point::new(lo, hi),
        ])
    }

    fn track_at_x(x: i64) -> Track {
        Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(x, 5_000_000), Point::new(x, 15_000_000)),
            width_nm: 200_000,
            layer: LayerId(0),
            net: NetId(1),
            locked: false,
        }
    }

    #[test]
    fn edge_clearance_matches_kicads_threshold_and_tolerance() {
        let mut board = Board::new();
        board.outline = Some(square(0, 20_000_000));
        // Track edge 0.5mm − 500nm from the right edge: passes; −501nm: fails.
        board
            .tracks
            .push(track_at_x(20_000_000 - 100_000 - 499_500));
        assert!(check_edge_clearance(&board).is_empty());
        board.tracks[0] = track_at_x(20_000_000 - 100_000 - 499_499);
        assert_eq!(check_edge_clearance(&board).len(), 1);
    }

    #[test]
    fn keepout_flags_overlap_only_on_its_layers() {
        let mut board = Board::new();
        board.rule_areas.push(RuleArea {
            id: RuleAreaId(0),
            name: "KO".to_string(),
            outline: square(8_000_000, 12_000_000),
            layers: vec![LayerId(0)],
            keepout: KeepoutFlags {
                tracks_allowed: false,
                vias_allowed: true,
                pads_allowed: true,
                copper_pour_allowed: true,
                footprints_allowed: true,
            },
        });
        board.tracks.push(track_at_x(12_050_000)); // copper reaches 11.95mm: overlaps
        assert_eq!(check_keepouts(&board).len(), 1);
        board.tracks[0] = track_at_x(12_150_000); // 0.05mm away: fine
        assert!(check_keepouts(&board).is_empty());
        board.tracks[0] = track_at_x(10_000_000);
        board.tracks[0].layer = LayerId(1);
        assert!(check_keepouts(&board).is_empty());
    }
}
