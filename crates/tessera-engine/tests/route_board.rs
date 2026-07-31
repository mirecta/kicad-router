use tessera_drc::check_clearance;
use tessera_engine::route_board;
use tessera_geom::{Circle, Point};
use tessera_model::{Board, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape};

fn trivial_two_net_board() -> Board {
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

    for (i, (x_from, x_to)) in [(0, 2_000_000), (0, 2_000_000)].into_iter().enumerate() {
        let net = NetId(u32::try_from(i).unwrap() + 1);
        let y = i64::try_from(i).unwrap() * 5_000_000;
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
            shape: PadShape::Circle(Circle::new(Point::new(x_from, y), 200_000)),
            layers: vec![LayerId(0)],
            net,
            locked: false,
            reference: None,
            number: None,
        });
        board.pads.push(Pad {
            id: PadId(u32::try_from(i * 2 + 1).unwrap()),
            shape: PadShape::Circle(Circle::new(Point::new(x_to, y), 200_000)),
            layers: vec![LayerId(0)],
            net,
            locked: false,
            reference: None,
            number: None,
        });
    }

    board
}

#[test]
fn routes_all_unrouted_nets_and_result_is_clearance_clean() {
    let mut board = trivial_two_net_board();

    let report = route_board(&mut board);

    assert_eq!(report.routed, 2, "both nets should route: {report:?}");
    assert!(report.failed.is_empty(), "unexpected failures: {report:?}");
    assert!(report.skipped.is_empty());
    assert!(!board.tracks.is_empty());

    let after = board.find_unrouted_connections();
    assert!(
        after.connections.is_empty(),
        "routed nets should no longer be reported as unrouted"
    );

    let violations = check_clearance(&board);
    assert!(
        violations.is_empty(),
        "routed board is not clearance-clean: {violations:?}"
    );
}
