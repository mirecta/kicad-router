//! Proves the global-routing integration (task: "wire global router as
//! corridor guide into tessera-detail") actually runs across multiple
//! simultaneous connections, not just a single isolated one — several
//! nets whose direct paths all cross the same region get negotiated
//! together and still all route to a clearance-clean result.

use tessera_drc::check_clearance;
use tessera_engine::route_board;
use tessera_geom::{Circle, Point, Segment};
use tessera_model::{
    Board, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape, Track, TrackId,
};

#[test]
fn several_nets_crossing_the_same_region_all_route_and_stay_clean() {
    let mut board = Board::new();
    board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
    board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
    board.net_classes.insert(
        "Default".to_string(),
        NetClass {
            name: "Default".to_string(),
            clearance_nm: 150_000,
            track_width_nm: 200_000,
            via_diameter_nm: 500_000,
            via_drill_nm: 250_000,
            diff_pair_track_width_nm: None,
            diff_pair_gap_nm: None,
            diff_pair_via_gap_nm: None,
        },
    );

    // Five parallel two-pin nets, all crossing left-to-right through
    // roughly the same region, stacked closely enough in y that global
    // negotiation has real (if abstract) congestion to reason about.
    for i in 0..5i64 {
        let net = NetId(u32::try_from(i).unwrap() + 1);
        let y = i * 800_000;
        board.nets.insert(
            net,
            Net {
                id: net,
                name: format!("NET{i}"),
                net_class: "Default".to_string(),
            },
        );
        board.pads.push(Pad {
            id: PadId(u32::try_from(i * 2).unwrap()),
            shape: PadShape::Circle(Circle::new(Point::new(0, y), 150_000)),
            layers: vec![LayerId(0)],
            net,
            locked: false,
            reference: None,
            number: None,
        });
        board.pads.push(Pad {
            id: PadId(u32::try_from(i * 2 + 1).unwrap()),
            shape: PadShape::Circle(Circle::new(Point::new(4_000_000, y), 150_000)),
            layers: vec![LayerId(0)],
            net,
            locked: false,
            reference: None,
            number: None,
        });
    }

    let report = route_board(&mut board);
    assert_eq!(report.routed, 5, "all five nets should route: {report:?}");
    assert!(report.failed.is_empty());

    let violations = check_clearance(&board);
    assert!(
        violations.is_empty(),
        "globally-negotiated routing is not clearance-clean: {violations:?}"
    );
}

#[test]
fn routes_around_a_pre_existing_locked_wall_and_stays_clean() {
    let mut board = Board::new();
    board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
    board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
    board.net_classes.insert(
        "Default".to_string(),
        NetClass {
            name: "Default".to_string(),
            clearance_nm: 150_000,
            track_width_nm: 200_000,
            via_diameter_nm: 500_000,
            via_drill_nm: 250_000,
            diff_pair_track_width_nm: None,
            diff_pair_gap_nm: None,
            diff_pair_via_gap_nm: None,
        },
    );

    let wall_net = NetId(1);
    board.nets.insert(
        wall_net,
        Net {
            id: wall_net,
            name: "WALL".to_string(),
            net_class: "Default".to_string(),
        },
    );
    // A locked, pre-existing track directly between the two target pads,
    // spanning well past the straight-line path in y — this is the fixed
    // geometry the obstacle-aware global grid should now steer around
    // before `tessera-detail` ever gets a local search window.
    board.tracks.push(Track {
        id: TrackId(0),
        segment: Segment::new(
            Point::new(3_000_000, -3_000_000),
            Point::new(3_000_000, 3_000_000),
        ),
        width_nm: 300_000,
        layer: LayerId(0),
        net: wall_net,
        locked: true,
    });

    let target_net = NetId(2);
    board.nets.insert(
        target_net,
        Net {
            id: target_net,
            name: "TARGET".to_string(),
            net_class: "Default".to_string(),
        },
    );
    board.pads.push(Pad {
        id: PadId(0),
        shape: PadShape::Circle(Circle::new(Point::new(0, 0), 200_000)),
        layers: vec![LayerId(0)],
        net: target_net,
        locked: false,
        reference: None,
        number: None,
    });
    board.pads.push(Pad {
        id: PadId(1),
        shape: PadShape::Circle(Circle::new(Point::new(6_000_000, 0), 200_000)),
        layers: vec![LayerId(0)],
        net: target_net,
        locked: false,
        reference: None,
        number: None,
    });

    let report = route_board(&mut board);
    assert_eq!(report.routed, 1, "the target net should route: {report:?}");
    assert!(report.failed.is_empty(), "unexpected failures: {report:?}");

    let violations = check_clearance(&board);
    assert!(
        violations.is_empty(),
        "routing around the locked wall is not clearance-clean: {violations:?}"
    );
}
