use tessera_drc::check_clearance;
use tessera_engine::route_board;
use tessera_geom::{Point, Polygon, Rect};
use tessera_model::{
    Board, CopperZone, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape, Via, ViaId,
};

const F: LayerId = LayerId(0);
const GND: NetId = NetId(3);
const STITCH: Point = Point::new(7_000_000, 0);

fn rect_pad(id: u32, net: NetId, center: Point, w: i64, h: i64) -> Pad {
    Pad {
        id: PadId(id),
        shape: PadShape::Rect(Rect::new(center, w, h)),
        layers: vec![F],
        net,
        locked: false,
        reference: None,
        number: None,
        drill_nm: None,
    }
}

/// One layer, a 1.2 mm channel along y=0 between two no-net walls
/// (x ∈ [2, 8] mm) that net B has to pass through, a GND pour over the
/// whole board, and a free GND via — the user's stitch — in the channel:
/// legal there itself, but leaving no room for a track beside it.
fn board_with_a_stitch_in_the_way() -> Board {
    let mut board = Board::new();
    board.layers.push(Layer::copper(F, "F.Cu"));
    let mut class = NetClass::default_placeholder();
    class.clearance_nm = 200_000;
    class.track_width_nm = 250_000;
    board.net_classes.insert("Default".to_string(), class);
    for (id, name) in [(2, "B"), (3, "GND")] {
        board.nets.insert(
            NetId(id),
            Net {
                id: NetId(id),
                name: name.to_string(),
                net_class: "Default".to_string(),
            },
        );
    }
    board
        .pads
        .push(rect_pad(2, NetId(2), Point::new(0, 0), 400_000, 400_000));
    board.pads.push(rect_pad(
        3,
        NetId(2),
        Point::new(10_000_000, 0),
        400_000,
        400_000,
    ));
    board.pads.push(rect_pad(
        4,
        NetId::NONE,
        Point::new(5_000_000, 3_500_000),
        6_000_000,
        5_800_000,
    ));
    board.pads.push(rect_pad(
        5,
        NetId::NONE,
        Point::new(5_000_000, -10_600_000),
        6_000_000,
        20_000_000,
    ));
    board.copper_zones.push(CopperZone {
        net: GND,
        layers: vec![F],
        outline: Some(Polygon::new(vec![
            Point::new(-5_000_000, -25_000_000),
            Point::new(15_000_000, -25_000_000),
            Point::new(15_000_000, 10_000_000),
            Point::new(-5_000_000, 10_000_000),
        ])),
        thermal_gap_nm: 0,
        thermal_bridge_nm: 0,
        min_thickness_nm: 0,
    });
    board.vias.push(Via {
        id: ViaId(0),
        position: STITCH,
        diameter_nm: 600_000,
        drill_nm: 300_000,
        from_layer: F,
        to_layer: F,
        net: GND,
        locked: false,
    });
    board
}

#[test]
fn a_stitching_via_in_the_way_steps_aside_within_its_pour() {
    let mut board = board_with_a_stitch_in_the_way();
    let report = route_board(&mut board);
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(length(&board), 10_000_000, "straight through the channel");
    assert_eq!(report.moved_vias.len(), 1, "{report:?}");
    let (net, from, to) = report.moved_vias[0];
    assert_eq!((net, from), (GND, STITCH));
    let gnd: Vec<&Via> = board.vias.iter().filter(|v| v.net == GND).collect();
    assert_eq!(gnd.len(), 1);
    assert_eq!(gnd[0].position, to);
    assert!(report.dropped_vias.is_empty());
    assert!(check_clearance(&board).is_empty());
}

fn length(board: &Board) -> i64 {
    board.tracks.iter().map(|t| t.segment.length_nm()).sum()
}

#[test]
fn with_fixed_vias_the_stitch_stays_and_the_track_goes_round() {
    let mut fixed = board_with_a_stitch_in_the_way();
    fixed.fixed_vias = true;
    let report = route_board(&mut fixed);
    assert!(report.moved_vias.is_empty());
    assert_eq!(fixed.vias.len(), 1);
    assert_eq!(fixed.vias[0].position, STITCH);

    let mut free = board_with_a_stitch_in_the_way();
    let _ = route_board(&mut free);
    assert!(
        length(&free) + 5_000_000 < length(&fixed),
        "{} vs {}",
        length(&free),
        length(&fixed)
    );
}

#[test]
fn a_stitching_via_with_room_stays_where_it_is() {
    let mut board = board_with_a_stitch_in_the_way();
    board.vias[0].position = Point::new(12_000_000, 5_000_000);
    let report = route_board(&mut board);
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(report.moved_vias.is_empty(), "{report:?}");
    assert_eq!(board.vias[0].id, ViaId(0));
    assert_eq!(board.vias[0].position, Point::new(12_000_000, 5_000_000));
}

#[test]
fn a_stitch_whose_spot_already_breaks_a_rule_stays_put() {
    let mut board = board_with_a_stitch_in_the_way();
    // narrow the channel to 0.8 mm: the via is too close to the walls
    board.pads[2].shape = PadShape::Rect(Rect::new(
        Point::new(5_000_000, 3_300_000),
        6_000_000,
        5_800_000,
    ));
    board.pads[3].shape = PadShape::Rect(Rect::new(
        Point::new(5_000_000, -10_400_000),
        6_000_000,
        20_000_000,
    ));
    let report = route_board(&mut board);
    assert!(report.moved_vias.is_empty(), "{report:?}");
    assert!(report.dropped_vias.is_empty());
    assert_eq!(board.vias[0].id, ViaId(0));
    assert_eq!(board.vias[0].position, STITCH);
}
