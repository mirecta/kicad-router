use tessera_geom::Circle;
use tessera_model::{Board, NetId, Pad, PadShape, Track, Via};

use crate::violation::{ClearanceViolation, ItemRef};

/// The clearance required between two items on `net_a` and `net_b`. Thin
/// re-export of [`Board::resolved_clearance_nm`], which moved there so
/// `tessera-detail` (which per plan §2.2 depends on `geom`+`model` only,
/// never `drc`) can resolve clearance without pulling in this crate's
/// violation-checking machinery. Kept here too since the M1 parity harness
/// and this crate's own tests already depend on this path.
#[must_use]
pub fn resolved_clearance_nm(board: &Board, net_a: NetId, net_b: NetId) -> Option<i64> {
    board.resolved_clearance_nm(net_a, net_b)
}

fn pad_circle(pad: &Pad) -> Circle {
    match &pad.shape {
        PadShape::Circle(c) => *c,
    }
}

fn via_circle(via: &Via) -> Circle {
    Circle::new(via.position, via.diameter_nm / 2)
}

fn via_layer_range(via: &Via) -> (u32, u32) {
    let a = via.from_layer.0;
    let b = via.to_layer.0;
    (a.min(b), a.max(b))
}

fn track_via_share_layer(track: &Track, via: &Via) -> bool {
    let (lo, hi) = via_layer_range(via);
    (lo..=hi).contains(&track.layer.0)
}

fn via_via_share_layer(a: &Via, b: &Via) -> bool {
    let (a_lo, a_hi) = via_layer_range(a);
    let (b_lo, b_hi) = via_layer_range(b);
    a_lo <= b_hi && b_lo <= a_hi
}

fn track_pad_share_layer(track: &Track, pad: &Pad) -> bool {
    pad.layers.contains(&track.layer)
}

fn via_pad_share_layer(via: &Via, pad: &Pad) -> bool {
    let (lo, hi) = via_layer_range(via);
    pad.layers.iter().any(|l| (lo..=hi).contains(&l.0))
}

fn pad_pad_share_layer(a: &Pad, b: &Pad) -> bool {
    a.layers.iter().any(|l| b.layers.contains(l))
}

/// All clearance violations on `board`, checked pairwise across tracks,
/// vias, and pads that (a) are on different nets, (b) share at least one
/// copper layer, and (c) are closer than their resolved net-class
/// clearance.
///
/// `O(n^2)` in item count — correct-first, not fast-first. Plan §4.3's
/// incremental spatial index is the intended fix once this needs to run on
/// real corpus-sized boards inside the routing hot loop; nothing here
/// should be optimised before that index exists and this is benchmarked
/// against it (plan §12: "benchmark before optimising").
///
/// A [`Track`]'s own half-width is added to the required clearance at each
/// track comparison — `tessera_geom::Segment`'s predicates treat a segment
/// as a zero-width line, and width is a PCB-domain concept that belongs
/// here, not in the geometry kernel.
#[must_use]
pub fn check_clearance(board: &Board) -> Vec<ClearanceViolation> {
    let mut violations = Vec::new();
    check_track_track(board, &mut violations);
    check_track_via(board, &mut violations);
    check_track_pad(board, &mut violations);
    check_via_via(board, &mut violations);
    check_via_pad(board, &mut violations);
    check_pad_pad(board, &mut violations);
    violations
}

fn check_track_track(board: &Board, violations: &mut Vec<ClearanceViolation>) {
    for (i, a) in board.tracks.iter().enumerate() {
        for b in &board.tracks[i + 1..] {
            if a.layer != b.layer {
                continue;
            }
            let Some(min_nm) = resolved_clearance_nm(board, a.net, b.net) else {
                continue;
            };
            let effective_min = min_nm + a.width_nm / 2 + b.width_nm / 2;
            if !a.segment.clears_segment(b.segment, effective_min) {
                violations.push(ClearanceViolation {
                    a: ItemRef::Track(a.id),
                    b: ItemRef::Track(b.id),
                    required_nm: min_nm,
                });
            }
        }
    }
}

fn check_track_via(board: &Board, violations: &mut Vec<ClearanceViolation>) {
    for t in &board.tracks {
        for v in &board.vias {
            if !track_via_share_layer(t, v) {
                continue;
            }
            let Some(min_nm) = resolved_clearance_nm(board, t.net, v.net) else {
                continue;
            };
            let effective_min = min_nm + t.width_nm / 2;
            if !via_circle(v).clears_segment(t.segment, effective_min) {
                violations.push(ClearanceViolation {
                    a: ItemRef::Track(t.id),
                    b: ItemRef::Via(v.id),
                    required_nm: min_nm,
                });
            }
        }
    }
}

fn check_track_pad(board: &Board, violations: &mut Vec<ClearanceViolation>) {
    for t in &board.tracks {
        for p in &board.pads {
            if !track_pad_share_layer(t, p) {
                continue;
            }
            let Some(min_nm) = resolved_clearance_nm(board, t.net, p.net) else {
                continue;
            };
            let effective_min = min_nm + t.width_nm / 2;
            if !pad_circle(p).clears_segment(t.segment, effective_min) {
                violations.push(ClearanceViolation {
                    a: ItemRef::Track(t.id),
                    b: ItemRef::Pad(p.id),
                    required_nm: min_nm,
                });
            }
        }
    }
}

fn check_via_via(board: &Board, violations: &mut Vec<ClearanceViolation>) {
    for (i, a) in board.vias.iter().enumerate() {
        for b in &board.vias[i + 1..] {
            if !via_via_share_layer(a, b) {
                continue;
            }
            let Some(min_nm) = resolved_clearance_nm(board, a.net, b.net) else {
                continue;
            };
            if !via_circle(a).clears_circle(via_circle(b), min_nm) {
                violations.push(ClearanceViolation {
                    a: ItemRef::Via(a.id),
                    b: ItemRef::Via(b.id),
                    required_nm: min_nm,
                });
            }
        }
    }
}

fn check_via_pad(board: &Board, violations: &mut Vec<ClearanceViolation>) {
    for v in &board.vias {
        for p in &board.pads {
            if !via_pad_share_layer(v, p) {
                continue;
            }
            let Some(min_nm) = resolved_clearance_nm(board, v.net, p.net) else {
                continue;
            };
            if !via_circle(v).clears_circle(pad_circle(p), min_nm) {
                violations.push(ClearanceViolation {
                    a: ItemRef::Via(v.id),
                    b: ItemRef::Pad(p.id),
                    required_nm: min_nm,
                });
            }
        }
    }
}

fn check_pad_pad(board: &Board, violations: &mut Vec<ClearanceViolation>) {
    for (i, a) in board.pads.iter().enumerate() {
        for b in &board.pads[i + 1..] {
            if !pad_pad_share_layer(a, b) {
                continue;
            }
            let Some(min_nm) = resolved_clearance_nm(board, a.net, b.net) else {
                continue;
            };
            if !pad_circle(a).clears_circle(pad_circle(b), min_nm) {
                violations.push(ClearanceViolation {
                    a: ItemRef::Pad(a.id),
                    b: ItemRef::Pad(b.id),
                    required_nm: min_nm,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::{Point, Segment};
    use tessera_model::{Layer, LayerId, Net, NetClass, PadId, TrackId, ViaId};

    fn two_net_board() -> (Board, NetId, NetId) {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));

        let mut class = NetClass::default_placeholder();
        class.clearance_nm = 200_000; // 0.2 mm
        board.net_classes.insert("Default".to_string(), class);

        let net_a = NetId(1);
        let net_b = NetId(2);
        board.nets.insert(
            net_a,
            Net {
                id: net_a,
                name: "A".to_string(),
                net_class: "Default".to_string(),
            },
        );
        board.nets.insert(
            net_b,
            Net {
                id: net_b,
                name: "B".to_string(),
                net_class: "Default".to_string(),
            },
        );

        (board, net_a, net_b)
    }

    #[test]
    fn same_net_tracks_never_violate() {
        let (mut board, net_a, _) = two_net_board();
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(0, 0), Point::new(1_000_000, 0)),
            width_nm: 250_000,
            layer: LayerId(0),
            net: net_a,
            locked: false,
        });
        board.tracks.push(Track {
            id: TrackId(1),
            // Overlapping the first track entirely, but same net.
            segment: Segment::new(Point::new(0, 0), Point::new(1_000_000, 0)),
            width_nm: 250_000,
            layer: LayerId(0),
            net: net_a,
            locked: false,
        });

        assert!(check_clearance(&board).is_empty());
    }

    #[test]
    fn different_net_tracks_too_close_violate() {
        let (mut board, net_a, net_b) = two_net_board();
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(0, 0), Point::new(1_000_000, 0)),
            width_nm: 200_000, // 0.2mm wide, so half-width 100_000nm
            layer: LayerId(0),
            net: net_a,
            locked: false,
        });
        board.tracks.push(Track {
            id: TrackId(1),
            // 300_000nm away on Y; required gap is clearance(200_000) +
            // half-widths (100_000 + 100_000) = 400_000, so this violates.
            segment: Segment::new(Point::new(0, 300_000), Point::new(1_000_000, 300_000)),
            width_nm: 200_000,
            layer: LayerId(0),
            net: net_b,
            locked: false,
        });

        let violations = check_clearance(&board);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].required_nm, 200_000);
    }

    #[test]
    fn different_net_tracks_far_enough_do_not_violate() {
        let (mut board, net_a, net_b) = two_net_board();
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(0, 0), Point::new(1_000_000, 0)),
            width_nm: 200_000,
            layer: LayerId(0),
            net: net_a,
            locked: false,
        });
        board.tracks.push(Track {
            id: TrackId(1),
            // 400_000nm away: exactly the required gap, so this clears.
            segment: Segment::new(Point::new(0, 400_000), Point::new(1_000_000, 400_000)),
            width_nm: 200_000,
            layer: LayerId(0),
            net: net_b,
            locked: false,
        });

        assert!(check_clearance(&board).is_empty());
    }

    #[test]
    fn tracks_on_different_layers_never_violate() {
        let (mut board, net_a, net_b) = two_net_board();
        board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(0, 0), Point::new(1_000_000, 0)),
            width_nm: 200_000,
            layer: LayerId(0),
            net: net_a,
            locked: false,
        });
        board.tracks.push(Track {
            id: TrackId(1),
            // Same geometry, different (copper) layer: no violation.
            segment: Segment::new(Point::new(0, 0), Point::new(1_000_000, 0)),
            width_nm: 200_000,
            layer: LayerId(1),
            net: net_b,
            locked: false,
        });

        assert!(check_clearance(&board).is_empty());
    }

    #[test]
    fn vias_too_close_violate() {
        let (mut board, net_a, net_b) = two_net_board();
        board.vias.push(Via {
            id: ViaId(0),
            position: Point::new(0, 0),
            diameter_nm: 600_000,
            drill_nm: 300_000,
            from_layer: LayerId(0),
            to_layer: LayerId(0),
            net: net_a,
            locked: false,
        });
        board.vias.push(Via {
            id: ViaId(1),
            // Centers 700_000nm apart; radii 300_000 each, so edge gap is
            // 100_000nm — less than the 200_000nm required clearance.
            position: Point::new(700_000, 0),
            diameter_nm: 600_000,
            drill_nm: 300_000,
            from_layer: LayerId(0),
            to_layer: LayerId(0),
            net: net_b,
            locked: false,
        });

        assert_eq!(check_clearance(&board).len(), 1);
    }

    #[test]
    fn pad_and_track_different_nets_too_close_violate() {
        let (mut board, net_a, net_b) = two_net_board();
        board.pads.push(Pad {
            id: PadId(0),
            shape: PadShape::Circle(Circle::new(Point::new(0, 0), 200_000)),
            layers: vec![LayerId(0)],
            net: net_a,
            locked: false,
        });
        board.tracks.push(Track {
            id: TrackId(0),
            // Pad edge at x=200_000; track starts at x=300_000, width
            // 100_000 (half-width 50_000): gap is 300_000-50_000-200_000 =
            // 50_000, less than the 200_000nm required clearance.
            segment: Segment::new(Point::new(300_000, 0), Point::new(1_000_000, 0)),
            width_nm: 100_000,
            layer: LayerId(0),
            net: net_b,
            locked: false,
        });

        assert_eq!(check_clearance(&board).len(), 1);
    }
}
