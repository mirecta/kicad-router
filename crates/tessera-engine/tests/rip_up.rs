use tessera_drc::check_clearance;
use tessera_engine::route_board;
use tessera_geom::{Point, Rect, Segment};
use tessera_model::{
    Board, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape, Track, TrackId,
};

const F: LayerId = LayerId(0);
const WALLS: NetId = NetId::NONE;
const OTHER: NetId = NetId(3);

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

/// One layer. Two walls (pads of an unrouted net) leave a 0.8mm channel
/// along y=0 — room for exactly one 0.25mm track at 0.2mm clearance. Net
/// B runs straight through it and, within its search window, has no other
/// way (the walls are no-net copper, never routed themselves). Net A runs
/// a little above and is routed first: its shortest path
/// also dips through the channel, but it *could* go over the top wall.
fn channel_board() -> Board {
    let mut board = Board::new();
    board.layers.push(Layer::copper(F, "F.Cu"));
    let mut class = NetClass::default_placeholder();
    class.clearance_nm = 200_000;
    class.track_width_nm = 250_000;
    board.net_classes.insert("Default".to_string(), class);
    for (id, name) in [(1, "A"), (2, "B"), (3, "OTHER")] {
        board.nets.insert(
            NetId(id),
            Net {
                id: NetId(id),
                name: name.to_string(),
                net_class: "Default".to_string(),
            },
        );
    }
    let a = NetId(1);
    let b = NetId(2);
    board
        .pads
        .push(rect_pad(0, a, Point::new(0, 1_000_000), 400_000, 400_000));
    board.pads.push(rect_pad(
        1,
        a,
        Point::new(10_000_000, 1_000_000),
        400_000,
        400_000,
    ));
    board
        .pads
        .push(rect_pad(2, b, Point::new(0, 0), 400_000, 400_000));
    board
        .pads
        .push(rect_pad(3, b, Point::new(10_000_000, 0), 400_000, 400_000));
    // Top wall y ∈ [0.4, 3.1]mm, bottom wall y ∈ [-20.4, -0.4]mm, x ∈ [2, 8]mm.
    board.pads.push(rect_pad(
        4,
        WALLS,
        Point::new(5_000_000, 1_750_000),
        6_000_000,
        2_700_000,
    ));
    board.pads.push(rect_pad(
        5,
        WALLS,
        Point::new(5_000_000, -10_400_000),
        6_000_000,
        20_000_000,
    ));
    board
}

#[test]
fn rip_up_lets_both_nets_through_a_one_track_channel() {
    let mut board = channel_board();
    let report = route_board(&mut board);
    assert_eq!(report.routed, 2, "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(check_clearance(&board).is_empty());
}

/// Plan §7.5.4: geometry that was on the board before routing is never a
/// rip-up candidate — here the user's own track fills the channel, so B
/// must fail and the track must survive untouched.
#[test]
fn existing_geometry_is_never_ripped_up() {
    let mut board = channel_board();
    board.pads.retain(|p| p.net != NetId(1));
    let user_track = Track {
        id: TrackId(0),
        segment: Segment::new(Point::new(1_500_000, 0), Point::new(8_500_000, 0)),
        width_nm: 250_000,
        layer: F,
        net: OTHER,
        locked: false,
    };
    board.tracks.push(user_track.clone());
    let report = route_board(&mut board);
    assert_eq!(report.failed, vec![NetId(2)]);
    assert!(board.tracks.contains(&user_track));
}
