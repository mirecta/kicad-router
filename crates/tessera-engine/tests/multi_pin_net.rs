//! M3: a 3-pin net should route via Steiner (MST) decomposition instead of
//! being reported as skipped — closing the gap M2 explicitly left open.

use tessera_drc::check_clearance;
use tessera_engine::route_board;
use tessera_geom::{Circle, Point};
use tessera_model::{Board, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape};

#[test]
fn three_pin_net_routes_via_steiner_decomposition() {
    let mut board = Board::new();
    board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
    board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
    board.net_classes.insert(
        "Default".to_string(),
        NetClass {
            name: "Default".to_string(),
            clearance_nm: 200_000,
            track_width_nm: 250_000,
            via_diameter_nm: 600_000,
            via_drill_nm: 300_000,
            diff_pair_track_width_nm: None,
            diff_pair_gap_nm: None,
            diff_pair_via_gap_nm: None,
        },
    );
    let net = NetId(1);
    board.nets.insert(
        net,
        Net {
            id: net,
            name: "NET1".to_string(),
            net_class: "Default".to_string(),
        },
    );

    for (i, (x, y)) in [(0, 0), (3_000_000, 0), (1_500_000, 2_500_000)]
        .into_iter()
        .enumerate()
    {
        board.pads.push(Pad {
            id: PadId(u32::try_from(i).unwrap()),
            shape: PadShape::Circle(Circle::new(Point::new(x, y), 200_000)),
            layers: vec![LayerId(0)],
            net,
            locked: false,
        });
    }

    let before = board.find_unrouted_connections();
    assert_eq!(
        before.multi_pin_nets.len(),
        1,
        "should be seen as multi-pin"
    );
    assert!(before.connections.is_empty());

    let report = route_board(&mut board);
    assert_eq!(
        report.routed, 1,
        "the 3-pin net should fully route: {report:?}"
    );
    assert!(report.failed.is_empty());
    assert!(
        !board.tracks.is_empty(),
        "expected at least some track segments from routing 2 MST edges"
    );

    let after = board.find_unrouted_connections();
    assert!(
        after.multi_pin_nets.is_empty() && after.connections.is_empty(),
        "net should no longer be reported as unrouted"
    );

    let violations = check_clearance(&board);
    assert!(
        violations.is_empty(),
        "routed multi-pin net is not clearance-clean: {violations:?}"
    );
}
