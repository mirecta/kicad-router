//! End-to-end check: route a connection that's forced to detour around an
//! obstacle, then verify the *result* is actually clearance-clean against
//! the rest of the board using `tessera-drc` — not just "A* returned a
//! path," but "the path it returned is real, valid copper."

use tessera_detail::route_connection;
use tessera_drc::check_clearance;
use tessera_geom::{Circle, Point, Segment};
use tessera_model::{
    Board, Connection, Endpoint, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape, Track,
    TrackId, Via, ViaId,
};

fn two_layer_board_with_class(clearance_nm: i64, track_width_nm: i64) -> Board {
    let mut board = Board::new();
    board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
    board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
    board.net_classes.insert(
        "Default".to_string(),
        NetClass {
            name: "Default".to_string(),
            clearance_nm,
            track_width_nm,
            via_diameter_nm: 600_000,
            via_drill_nm: 300_000,
            diff_pair_track_width_nm: None,
            diff_pair_gap_nm: None,
            diff_pair_via_gap_nm: None,
        },
    );
    board
}

fn add_net(board: &mut Board, id: u32, name: &str) -> NetId {
    let net = NetId(id);
    board.nets.insert(
        net,
        Net {
            id: net,
            name: name.to_string(),
            net_class: "Default".to_string(),
        },
    );
    net
}

#[test]
fn routes_straight_line_with_no_obstacles() {
    let mut board = two_layer_board_with_class(200_000, 250_000);
    let net_a = add_net(&mut board, 1, "A");
    board.pads.push(Pad {
        id: PadId(0),
        shape: PadShape::Circle(Circle::new(Point::new(0, 0), 200_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
    });
    board.pads.push(Pad {
        id: PadId(1),
        shape: PadShape::Circle(Circle::new(Point::new(2_000_000, 0), 200_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
    });

    let connection = Connection {
        net: net_a,
        from: Endpoint {
            position: Point::new(0, 0),
            layers: vec![LayerId(0)],
        },
        to: Endpoint {
            position: Point::new(2_000_000, 0),
            layers: vec![LayerId(0)],
        },
    };

    let routed = route_connection(&board, &connection, &[]).expect("should find a path");
    assert!(!routed.segments.is_empty());
    assert!(routed.vias.is_empty(), "no layer change needed here");
}

#[test]
fn detours_around_obstacle_and_result_is_clearance_clean() {
    let mut board = two_layer_board_with_class(200_000, 250_000);
    let net_a = add_net(&mut board, 1, "A");
    let net_b = add_net(&mut board, 2, "B");

    board.pads.push(Pad {
        id: PadId(0),
        shape: PadShape::Circle(Circle::new(Point::new(0, 0), 200_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
    });
    board.pads.push(Pad {
        id: PadId(1),
        shape: PadShape::Circle(Circle::new(Point::new(3_000_000, 0), 200_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
    });

    // A net-B track directly between the two net-A pads, wider than the
    // pad-to-pad line, forcing a detour (or a layer change) rather than a
    // straight shot.
    board.tracks.push(Track {
        id: TrackId(0),
        segment: Segment::new(
            Point::new(1_500_000, -1_500_000),
            Point::new(1_500_000, 1_500_000),
        ),
        width_nm: 250_000,
        layer: LayerId(0),
        net: net_b,
        locked: false,
    });

    let connection = Connection {
        net: net_a,
        from: Endpoint {
            position: Point::new(0, 0),
            layers: vec![LayerId(0)],
        },
        to: Endpoint {
            position: Point::new(3_000_000, 0),
            layers: vec![LayerId(0)],
        },
    };

    let routed =
        route_connection(&board, &connection, &[]).expect("should route around the obstacle");
    assert!(!routed.segments.is_empty());

    // Commit the routed path into a copy of the board (as real Track/Via
    // items) and confirm tessera-drc finds zero violations — the router's
    // internal notion of "clear" must actually agree with the DRC engine's,
    // not just with itself.
    let mut committed = board.clone();
    for (i, (segment, layer)) in routed.segments.iter().enumerate() {
        committed.tracks.push(Track {
            id: TrackId(100 + u32::try_from(i).unwrap_or(u32::MAX)),
            segment: *segment,
            width_nm: 250_000,
            layer: *layer,
            net: net_a,
            locked: false,
        });
    }
    for (i, position) in routed.vias.iter().enumerate() {
        committed.vias.push(Via {
            id: ViaId(100 + u32::try_from(i).unwrap_or(u32::MAX)),
            position: *position,
            diameter_nm: 600_000,
            drill_nm: 300_000,
            from_layer: LayerId(0),
            to_layer: LayerId(1),
            net: net_a,
            locked: false,
        });
    }

    let violations = check_clearance(&committed);
    assert!(
        violations.is_empty(),
        "routed path is not clearance-clean: {violations:?}"
    );
}
