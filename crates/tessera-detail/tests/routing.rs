//! End-to-end check: route a connection that's forced to detour around an
//! obstacle, then verify the *result* is actually clearance-clean against
//! the rest of the board using `tessera-drc` — not just "A* returned a
//! path," but "the path it returned is real, valid copper."

use tessera_detail::route_connection;
use tessera_drc::{
    check_clearance, check_edge_clearance, check_hole_clearance, check_keepouts,
    check_zone_clearance,
};
use tessera_geom::{Circle, Point, Polygon, Rect, Segment};
use tessera_model::{
    Board, Connection, Endpoint, Hole, HoleId, KeepoutFlags, Layer, LayerId, Net, NetClass, NetId,
    Pad, PadId, PadShape, RuleArea, RuleAreaId, Track, TrackId, Via, ViaId, ZoneFill,
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
        reference: None,
        number: None,
        drill_nm: None,
    });
    board.pads.push(Pad {
        id: PadId(1),
        shape: PadShape::Circle(Circle::new(Point::new(2_000_000, 0), 200_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
        reference: None,
        number: None,
        drill_nm: None,
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
        reference: None,
        number: None,
        drill_nm: None,
    });
    board.pads.push(Pad {
        id: PadId(1),
        shape: PadShape::Circle(Circle::new(Point::new(3_000_000, 0), 200_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
        reference: None,
        number: None,
        drill_nm: None,
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

#[test]
fn detours_around_a_rect_pad_obstacle_and_result_is_clearance_clean() {
    let mut board = two_layer_board_with_class(200_000, 250_000);
    let net_a = add_net(&mut board, 1, "A");
    let net_b = add_net(&mut board, 2, "B");

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
    board.pads.push(Pad {
        id: PadId(1),
        shape: PadShape::Circle(Circle::new(Point::new(3_000_000, 0), 200_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
        reference: None,
        number: None,
        drill_nm: None,
    });
    // A wide rectangular pad on a different net, straddling the direct
    // path between the two net-A pads.
    board.pads.push(Pad {
        id: PadId(2),
        shape: PadShape::Rect(Rect::new(Point::new(1_500_000, 0), 400_000, 3_000_000)),
        layers: vec![LayerId(0)],
        net: net_b,
        locked: false,
        reference: None,
        number: None,
        drill_nm: None,
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

    let routed = route_connection(&board, &connection, &[])
        .expect("should route around the rect pad obstacle");
    assert!(!routed.segments.is_empty());

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
        "routed path is not clearance-clean around the rect pad: {violations:?}"
    );
}

#[test]
fn waypoint_hint_hard_constrains_the_search_to_the_corridor() {
    // Nothing here physically obstructs the direct straight-line path
    // between the two pads — the only reason to prefer the bent waypoint
    // route is the hard corridor constraint itself. Before that
    // constraint existed, waypoints only widened the search window's
    // bounding box, so an unobstructed shorter path elsewhere in that box
    // (like the direct line) would still win.
    let mut board = two_layer_board_with_class(150_000, 200_000);
    let net_a = add_net(&mut board, 1, "A");

    board.pads.push(Pad {
        id: PadId(0),
        shape: PadShape::Circle(Circle::new(Point::new(0, 0), 150_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
        reference: None,
        number: None,
        drill_nm: None,
    });
    board.pads.push(Pad {
        id: PadId(1),
        shape: PadShape::Circle(Circle::new(Point::new(6_000_000, 0), 150_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
        reference: None,
        number: None,
        drill_nm: None,
    });

    let connection = Connection {
        net: net_a,
        from: Endpoint {
            position: Point::new(0, 0),
            layers: vec![LayerId(0)],
        },
        to: Endpoint {
            position: Point::new(6_000_000, 0),
            layers: vec![LayerId(0)],
        },
    };
    let waypoints = [
        Point::new(2_000_000, 2_500_000),
        Point::new(4_000_000, 2_500_000),
    ];

    let routed = route_connection(&board, &connection, &waypoints)
        .expect("should route by following the corridor");
    assert!(!routed.segments.is_empty());

    let polyline = [
        Point::new(0, 0),
        waypoints[0],
        waypoints[1],
        Point::new(6_000_000, 0),
    ];
    let corridor_segments: Vec<Segment> = polyline
        .windows(2)
        .map(|w| Segment::new(w[0], w[1]))
        .collect();
    // The corridor's own half-width plus slack for grid quantization
    // (CELL_NM) and diagonal-step rounding — generous, but far tighter
    // than the direct line's ~2.5mm deviation from the waypoint bend, so
    // this still fails if the router ignored the corridor entirely.
    let tolerance_nm = 2_000_000;
    for (segment, _layer) in &routed.segments {
        for point in [segment.a, segment.b] {
            let near_corridor = corridor_segments
                .iter()
                .any(|s| !s.clears_point(point, tolerance_nm));
            assert!(
                near_corridor,
                "routed point {point:?} strayed too far from the waypoint corridor"
            );
        }
    }
}

#[test]
fn detours_around_a_mounting_hole_by_the_boards_hole_clearance() {
    let mut board = two_layer_board_with_class(200_000, 250_000);
    board.rules.min_hole_clearance_nm = 400_000;
    let net_a = add_net(&mut board, 1, "A");
    for (id, x) in [(0, 0), (1, 6_000_000)] {
        board.pads.push(Pad {
            id: PadId(id),
            shape: PadShape::Circle(Circle::new(Point::new(x, 0), 200_000)),
            layers: vec![LayerId(0)],
            net: net_a,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });
    }
    // A 3.2mm mounting hole sitting right on the direct path.
    board.npth_holes.push(Hole {
        id: HoleId(0),
        position: Point::new(3_000_000, 0),
        diameter_nm: 3_200_000,
    });

    let connection = Connection {
        net: net_a,
        from: Endpoint {
            position: Point::new(0, 0),
            layers: vec![LayerId(0)],
        },
        to: Endpoint {
            position: Point::new(6_000_000, 0),
            layers: vec![LayerId(0)],
        },
    };
    let routed = route_connection(&board, &connection, &[]).expect("should route around the hole");

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
    let violations = check_hole_clearance(&committed);
    assert!(
        violations.is_empty(),
        "routed through the hole's clearance: {violations:?}"
    );
}

#[test]
fn detours_around_another_nets_zone_fill_by_the_zone_clearance() {
    let mut board = two_layer_board_with_class(200_000, 250_000);
    let net_a = add_net(&mut board, 1, "A");
    let net_b = add_net(&mut board, 2, "B");
    for (id, x) in [(0, 0), (1, 6_000_000)] {
        board.pads.push(Pad {
            id: PadId(id),
            shape: PadShape::Circle(Circle::new(Point::new(x, 0), 200_000)),
            layers: vec![LayerId(0)],
            net: net_a,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });
    }
    // Net B's pour across the direct path on F.Cu, with a zone clearance
    // larger than the class clearance.
    board.zone_fills.push(ZoneFill {
        net: net_b,
        layer: LayerId(0),
        clearance_nm: 400_000,
        inflation_nm: 0,
        polygon: Polygon::new(vec![
            Point::new(2_000_000, -1_500_000),
            Point::new(4_000_000, -1_500_000),
            Point::new(4_000_000, 1_500_000),
            Point::new(2_000_000, 1_500_000),
        ]),
    });

    let connection = Connection {
        net: net_a,
        from: Endpoint {
            position: Point::new(0, 0),
            layers: vec![LayerId(0)],
        },
        to: Endpoint {
            position: Point::new(6_000_000, 0),
            layers: vec![LayerId(0)],
        },
    };
    let routed = route_connection(&board, &connection, &[]).expect("should route around the pour");

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
    let violations = check_zone_clearance(&committed);
    assert!(
        violations.is_empty(),
        "routed through the pour's clearance: {violations:?}"
    );
}

fn two_pad_net_a(board: &mut Board, y: i64) -> Connection {
    let net_a = add_net(board, 1, "A");
    for (id, x) in [(0, 1_000_000), (1, 9_000_000)] {
        board.pads.push(Pad {
            id: PadId(id),
            shape: PadShape::Circle(Circle::new(Point::new(x, y), 200_000)),
            layers: vec![LayerId(0)],
            net: net_a,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });
    }
    Connection {
        net: net_a,
        from: Endpoint {
            position: Point::new(1_000_000, y),
            layers: vec![LayerId(0)],
        },
        to: Endpoint {
            position: Point::new(9_000_000, y),
            layers: vec![LayerId(0)],
        },
    }
}

fn commit_tracks(board: &Board, routed: &tessera_detail::RoutedPath, net: NetId) -> Board {
    let mut committed = board.clone();
    for (i, (segment, layer)) in routed.segments.iter().enumerate() {
        committed.tracks.push(Track {
            id: TrackId(100 + u32::try_from(i).unwrap_or(u32::MAX)),
            segment: *segment,
            width_nm: 250_000,
            layer: *layer,
            net,
            locked: false,
        });
    }
    committed
}

#[test]
fn keeps_board_edge_clearance_around_a_notch_in_the_outline() {
    let mut board = two_layer_board_with_class(200_000, 250_000);
    // A 10x10mm board whose bottom edge has a notch up to y=1.2mm between
    // x=4 and x=6 — right across the straight path at y=0.8mm.
    board.outline = Some(Polygon::new(vec![
        Point::new(0, 0),
        Point::new(4_000_000, 0),
        Point::new(4_000_000, 1_200_000),
        Point::new(6_000_000, 1_200_000),
        Point::new(6_000_000, 0),
        Point::new(10_000_000, 0),
        Point::new(10_000_000, 10_000_000),
        Point::new(0, 10_000_000),
    ]));
    board.rules.min_copper_edge_clearance_nm = 300_000;
    let connection = two_pad_net_a(&mut board, 800_000);
    let routed = route_connection(&board, &connection, &[]).expect("should route around the notch");
    let committed = commit_tracks(&board, &routed, connection.net);
    let violations = check_edge_clearance(&committed);
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn routes_around_a_keepout_that_forbids_tracks() {
    let mut board = two_layer_board_with_class(200_000, 250_000);
    let connection = two_pad_net_a(&mut board, 5_000_000);
    board.rule_areas.push(RuleArea {
        id: RuleAreaId(0),
        name: "KO".to_string(),
        outline: Polygon::new(vec![
            Point::new(4_000_000, 3_500_000),
            Point::new(6_000_000, 3_500_000),
            Point::new(6_000_000, 6_500_000),
            Point::new(4_000_000, 6_500_000),
        ]),
        layers: vec![LayerId(0), LayerId(1)],
        keepout: KeepoutFlags {
            tracks_allowed: false,
            vias_allowed: false,
            pads_allowed: true,
            copper_pour_allowed: true,
            footprints_allowed: true,
        },
    });
    let routed =
        route_connection(&board, &connection, &[]).expect("should route around the keepout");
    let committed = commit_tracks(&board, &routed, connection.net);
    let violations = check_keepouts(&committed);
    assert!(violations.is_empty(), "{violations:?}");
    assert!(check_clearance(&committed).is_empty());
}

/// A 0.3mm pin between two neighbours at 0.5mm pitch, whose centre isn't on
/// the grid the window's corner would give: the grid is aligned to the
/// pin, so the search has a legal place to start.
#[test]
fn routes_out_of_a_fine_pitch_pin_whose_centre_is_off_grid() {
    let mut board = two_layer_board_with_class(200_000, 200_000);
    let net_a = add_net(&mut board, 1, "A");
    let other = add_net(&mut board, 2, "B");
    let pin = |id: u32, x: i64, net: NetId| Pad {
        id: PadId(id),
        shape: PadShape::Rect(Rect::new(Point::new(x, 3_037_000), 300_000, 1_150_000)),
        layers: vec![LayerId(0)],
        net,
        locked: false,
        reference: None,
        number: None,
        drill_nm: None,
    };
    let x0 = 5_043_000;
    board.pads.push(pin(0, x0 - 500_000, other));
    board.pads.push(pin(1, x0, net_a));
    board.pads.push(pin(2, x0 + 500_000, other));
    board.pads.push(Pad {
        id: PadId(3),
        shape: PadShape::Circle(Circle::new(Point::new(0, 0), 300_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
        reference: None,
        number: None,
        drill_nm: None,
    });
    let connection = Connection {
        net: net_a,
        from: Endpoint {
            position: Point::new(x0, 3_037_000),
            layers: vec![LayerId(0)],
        },
        to: Endpoint {
            position: Point::new(0, 0),
            layers: vec![LayerId(0)],
        },
    };
    let routed = route_connection(&board, &connection, &[]).expect("should escape the pin");
    let committed = commit_tracks(&board, &routed, net_a);
    assert!(check_clearance(&committed).is_empty());
}
