use tessera_drc::{check_clearance, check_hole_to_hole, check_via_annular_width};
use tessera_engine::route_board;
use tessera_geom::{Circle, Point, Rect};
use tessera_model::{Board, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape, Plane};

const F: LayerId = LayerId(0);
const IN1: LayerId = LayerId(1);
const IN2: LayerId = LayerId(2);
const B: LayerId = LayerId(3);

fn pad(id: u32, net: NetId, at: Point, layers: Vec<LayerId>, drill_nm: Option<i64>) -> Pad {
    Pad {
        id: PadId(id),
        shape: PadShape::Rect(Rect::new(at, 800_000, 800_000)),
        layers,
        net,
        locked: false,
        reference: None,
        number: None,
        drill_nm,
    }
}

fn four_layer_board() -> Board {
    let mut board = Board::new();
    for (id, name) in [(F, "F.Cu"), (IN1, "In1.Cu"), (IN2, "In2.Cu"), (B, "B.Cu")] {
        board.layers.push(Layer::copper(id, name));
    }
    let mut class = NetClass::default_placeholder();
    class.clearance_nm = 200_000;
    class.track_width_nm = 250_000;
    board.net_classes.insert("Default".to_string(), class);
    for (id, name) in [(1, "SIG"), (2, "GND")] {
        board.nets.insert(
            NetId(id),
            Net {
                id: NetId(id),
                name: name.to_string(),
                net_class: "Default".to_string(),
            },
        );
    }
    let all = vec![F, IN1, IN2, B];
    board
        .pads
        .push(pad(0, NetId(1), Point::new(0, 0), vec![F], None));
    board
        .pads
        .push(pad(1, NetId(1), Point::new(6_000_000, 0), vec![F], None));
    // GND: one surface pad (needs a fanout), one through-hole pad (doesn't).
    board.pads.push(pad(
        2,
        NetId(2),
        Point::new(3_000_000, 3_000_000),
        vec![F],
        None,
    ));
    board.pads.push(pad(
        3,
        NetId(2),
        Point::new(3_000_000, -3_000_000),
        all,
        Some(400_000),
    ));
    board.planes.push(Plane {
        layer: IN2,
        net: NetId(2),
    });
    board
}

#[test]
fn signals_stay_off_plane_layers_and_plane_pads_get_a_fanout_via() {
    let mut board = four_layer_board();
    board.planes.push(Plane {
        layer: IN1,
        net: NetId(2),
    });
    let report = route_board(&mut board);

    assert_eq!(report.routed, 1, "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(report.fanouts, 1, "only the surface GND pad needs one");
    assert!(board.tracks.iter().all(|t| t.layer == F || t.layer == B));
    assert!(board
        .tracks
        .iter()
        .all(|t| t.net == NetId(1) || t.net == NetId(2)));

    let gnd_vias: Vec<_> = board.vias.iter().filter(|v| v.net == NetId(2)).collect();
    assert_eq!(gnd_vias.len(), 1);
    // The fanout via sits next to the surface pad, outside its copper.
    let surface_pad = Rect::new(Point::new(3_000_000, 3_000_000), 800_000, 800_000);
    let via = Circle::new(gnd_vias[0].position, gnd_vias[0].diameter_nm / 2);
    assert!(surface_pad.clears_circle(via, 200_000));
    assert!(gnd_vias[0].position.x.abs_diff(3_000_000) < 2_000_000);

    assert!(check_clearance(&board).is_empty());
    assert!(check_hole_to_hole(&board).is_empty());
    assert!(check_via_annular_width(&board).is_empty());
}

#[test]
fn a_plane_pad_already_wired_by_the_user_is_left_alone() {
    let mut board = four_layer_board();
    board.vias.push(tessera_model::Via {
        id: tessera_model::ViaId(0),
        position: Point::new(3_000_000, 3_000_000),
        diameter_nm: 600_000,
        drill_nm: 300_000,
        from_layer: F,
        to_layer: B,
        net: NetId(2),
        locked: false,
    });
    let report = route_board(&mut board);
    assert_eq!(report.fanouts, 0);
    assert_eq!(board.vias.iter().filter(|v| v.net == NetId(2)).count(), 1);
}
