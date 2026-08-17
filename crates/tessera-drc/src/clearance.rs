use tessera_geom::{Circle, Segment};
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

// pub(crate): reused by custom_rules.rs's pairwise `.kicad_dru` clearance
// check, which needs the exact same geometry/layer-sharing logic as this
// module's net-class check — only how the required distance is *resolved*
// differs between the two, not how a pair is found or measured.

pub(crate) fn via_circle(via: &Via) -> Circle {
    Circle::new(via.position, via.diameter_nm / 2)
}

/// True iff `pad`'s own copper shape clears `segment` by at least
/// `min_nm` — shape-dispatching so callers don't need to know or care
/// which concrete `PadShape` this is.
pub(crate) fn pad_clears_segment(pad: &Pad, segment: Segment, min_nm: i64) -> bool {
    match &pad.shape {
        PadShape::Circle(c) => c.clears_segment(segment, min_nm),
        PadShape::Rect(r) => r.clears_segment(segment, min_nm),
        PadShape::Oval(o) => o.clears_segment(segment, min_nm),
    }
}

/// True iff `pad`'s own copper shape clears `circle` (a via, or another
/// pad already known to be circular) by at least `min_nm`.
pub(crate) fn pad_clears_circle(pad: &Pad, circle: Circle, min_nm: i64) -> bool {
    match &pad.shape {
        PadShape::Circle(c) => c.clears_circle(circle, min_nm),
        PadShape::Rect(r) => r.clears_circle(circle, min_nm),
        PadShape::Oval(o) => o.clears_circle(circle, min_nm),
    }
}

/// True iff `a` and `b`'s own copper shapes clear each other by at
/// least `min_nm` — every shape-pair combination `PadShape` currently
/// has. Mixed pairings are routed through whichever side's own
/// `clears_*` method already exists for the other shape (`Rect::
/// clears_circle`, `Oval::clears_circle`, `Oval::clears_rect`) — each of
/// those predicates is already symmetric in what it measures, just not
/// in which type calls it.
pub(crate) fn pad_clears_pad(a: &Pad, b: &Pad, min_nm: i64) -> bool {
    match (&a.shape, &b.shape) {
        (PadShape::Circle(ca), PadShape::Circle(cb)) => ca.clears_circle(*cb, min_nm),
        (PadShape::Rect(ra), PadShape::Rect(rb)) => ra.clears_rect(*rb, min_nm),
        (PadShape::Oval(oa), PadShape::Oval(ob)) => oa.clears_oval(*ob, min_nm),
        (PadShape::Circle(ca), PadShape::Rect(rb)) => rb.clears_circle(*ca, min_nm),
        (PadShape::Rect(ra), PadShape::Circle(cb)) => ra.clears_circle(*cb, min_nm),
        (PadShape::Circle(ca), PadShape::Oval(ob)) => ob.clears_circle(*ca, min_nm),
        (PadShape::Oval(oa), PadShape::Circle(cb)) => oa.clears_circle(*cb, min_nm),
        (PadShape::Rect(ra), PadShape::Oval(ob)) => ob.clears_rect(*ra, min_nm),
        (PadShape::Oval(oa), PadShape::Rect(rb)) => oa.clears_rect(*rb, min_nm),
    }
}

fn via_layer_range(via: &Via) -> (u32, u32) {
    let a = via.from_layer.0;
    let b = via.to_layer.0;
    (a.min(b), a.max(b))
}

pub(crate) fn track_via_share_layer(track: &Track, via: &Via) -> bool {
    let (lo, hi) = via_layer_range(via);
    (lo..=hi).contains(&track.layer.0)
}

pub(crate) fn via_via_share_layer(a: &Via, b: &Via) -> bool {
    let (a_lo, a_hi) = via_layer_range(a);
    let (b_lo, b_hi) = via_layer_range(b);
    a_lo <= b_hi && b_lo <= a_hi
}

pub(crate) fn track_pad_share_layer(track: &Track, pad: &Pad) -> bool {
    pad.layers.contains(&track.layer)
}

pub(crate) fn via_pad_share_layer(via: &Via, pad: &Pad) -> bool {
    let (lo, hi) = via_layer_range(via);
    pad.layers.iter().any(|l| (lo..=hi).contains(&l.0))
}

pub(crate) fn pad_pad_share_layer(a: &Pad, b: &Pad) -> bool {
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
            if !pad_clears_segment(p, t.segment, effective_min) {
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
            if !pad_clears_circle(p, via_circle(v), min_nm) {
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
            if !pad_clears_pad(a, b, min_nm) {
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
            reference: None,
            number: None,
            drill_nm: None,
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

    #[test]
    fn rect_pad_and_track_diagonal_gap_matches_kicad_cli() {
        // Verified against real kicad-cli: a 2x1mm rect pad centred at
        // the origin (right edge x=1.0mm, top edge y=0.5mm) and a track
        // whose nearest point is diagonally past the pad's corner
        // reports actual clearance 0.4099mm (centreline distance
        // 0.5099mm minus the track's 0.1mm half-width) — see
        // docs/DECISIONS.md's pad-shape entry for the fixture.
        let (mut board, net_a, net_b) = two_net_board();
        board.pads.push(Pad {
            id: PadId(0),
            shape: PadShape::Rect(tessera_geom::Rect::new(
                Point::new(0, 0),
                2_000_000,
                1_000_000,
            )),
            layers: vec![LayerId(0)],
            net: net_a,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(
                Point::new(1_500_000, 600_000),
                Point::new(3_000_000, 600_000),
            ),
            width_nm: 200_000,
            layer: LayerId(0),
            net: net_b,
            locked: false,
        });

        let violations = check_clearance(&board);
        assert_eq!(
            violations.len(),
            0,
            "two_net_board's 0.2mm net-class default clears this ~0.41mm gap"
        );

        // Tighten past the real (~0.41mm) gap to confirm it does violate
        // once the required clearance exceeds it.
        board.net_classes.get_mut("Default").unwrap().clearance_nm = 500_000; // 0.5mm
        let violations = check_clearance(&board);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].required_nm, 500_000);
    }

    #[test]
    fn rect_rect_edge_only_gap_matches_kicad_cli() {
        // Verified against real kicad-cli: a 2x1mm rect at the origin
        // and a 1x1mm rect centred at (1.503, 0.504) overlap on the Y
        // axis but not X — actual clearance is the pure X-axis gap,
        // 0.0030mm, not a diagonal corner distance.
        let (mut board, net_a, net_b) = two_net_board();
        board.net_classes.get_mut("Default").unwrap().clearance_nm = 1_000_000; // 1mm
        board.pads.push(Pad {
            id: PadId(0),
            shape: PadShape::Rect(tessera_geom::Rect::new(
                Point::new(0, 0),
                2_000_000,
                1_000_000,
            )),
            layers: vec![LayerId(0)],
            net: net_a,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });
        board.pads.push(Pad {
            id: PadId(1),
            shape: PadShape::Rect(tessera_geom::Rect::new(
                Point::new(1_503_000, 504_000),
                1_000_000,
                1_000_000,
            )),
            layers: vec![LayerId(0)],
            net: net_b,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });

        let violations = check_clearance(&board);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].required_nm, 1_000_000);
    }

    #[test]
    fn a_rect_pad_and_a_circular_pad_clear_each_other_correctly() {
        // Mixed-shape pad-pad clearance: a 1mm-radius circle 2mm away
        // (centre to centre) from a 1x1mm rect's edge — exact gap is
        // 2_000_000 - 500_000 (rect half-width) - 1_000_000 (circle
        // radius) = 500_000nm.
        let (mut board, net_a, net_b) = two_net_board();
        board.pads.push(Pad {
            id: PadId(0),
            shape: PadShape::Rect(tessera_geom::Rect::new(
                Point::new(0, 0),
                1_000_000,
                1_000_000,
            )),
            layers: vec![LayerId(0)],
            net: net_a,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });
        board.pads.push(Pad {
            id: PadId(1),
            shape: PadShape::Circle(Circle::new(Point::new(2_000_000, 0), 1_000_000)),
            layers: vec![LayerId(0)],
            net: net_b,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });

        board.net_classes.get_mut("Default").unwrap().clearance_nm = 500_000;
        assert!(check_clearance(&board).is_empty());
        board.net_classes.get_mut("Default").unwrap().clearance_nm = 500_001;
        assert_eq!(check_clearance(&board).len(), 1);
    }

    #[test]
    fn oval_pad_and_track_clearance_matches_the_thick_segment_model() {
        // A 2x1mm oval (spine (-500_000,0)-(500_000,0), radius 500_000)
        // and a track whose closest point is straight out from the
        // spine's end — gap is centreline distance minus the oval's own
        // radius minus the track's half-width, the same "thick segment"
        // model `Oval` is built on (see tessera-geom::Oval's docs).
        let (mut board, net_a, net_b) = two_net_board();
        board.pads.push(Pad {
            id: PadId(0),
            shape: PadShape::Oval(tessera_geom::Oval::new(
                Point::new(0, 0),
                2_000_000,
                1_000_000,
            )),
            layers: vec![LayerId(0)],
            net: net_a,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });
        board.tracks.push(Track {
            id: TrackId(0),
            // Straight out from the spine end (500_000, 0): track runs
            // along x=900_000, width 200_000 (half-width 100_000). Gap
            // to the oval's edge: 900_000 - 500_000(spine end) -
            // 500_000(radius) - 100_000(half-width) = -200_000 — a real
            // overlap, so this violates regardless of net-class clearance.
            segment: Segment::new(
                Point::new(900_000, -1_000_000),
                Point::new(900_000, 1_000_000),
            ),
            width_nm: 200_000,
            layer: LayerId(0),
            net: net_b,
            locked: false,
        });

        assert_eq!(check_clearance(&board).len(), 1);
    }

    #[test]
    fn mixed_oval_and_rect_pad_pad_clearance() {
        let (mut board, net_a, net_b) = two_net_board();
        board.net_classes.get_mut("Default").unwrap().clearance_nm = 500_000;
        board.pads.push(Pad {
            id: PadId(0),
            shape: PadShape::Oval(tessera_geom::Oval::new(
                Point::new(0, 0),
                2_000_000,
                1_000_000,
            )), // spine end at 500_000, radius 500_000
            layers: vec![LayerId(0)],
            net: net_a,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });
        board.pads.push(Pad {
            id: PadId(1),
            shape: PadShape::Rect(tessera_geom::Rect::new(
                Point::new(2_000_000, 0),
                1_000_000,
                1_000_000,
            )), // half-width 500_000, left edge at 1_500_000
            layers: vec![LayerId(0)],
            net: net_b,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });

        // Gap: rect left edge (1_500_000) - oval spine end (500_000) -
        // oval radius (500_000) = 500_000.
        assert!(check_clearance(&board).is_empty());
        board.net_classes.get_mut("Default").unwrap().clearance_nm = 500_001;
        assert_eq!(check_clearance(&board).len(), 1);
    }
}
