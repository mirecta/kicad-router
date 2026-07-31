//! Round-trip test: `fixture::write_fixture` then `parser::parse_board`
//! should recover an equivalent board, exercising the parser against
//! exactly what this crate's own writer produces (see both modules' scope
//! notes — real-world files may use features neither side supports yet).

use tessera_geom::{Circle, Point, Segment};
use tessera_io_kicad::fixture::write_fixture;
use tessera_io_kicad::parser::parse_board;
use tessera_model::{
    Board, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape, Track, TrackId, Via, ViaId,
};

fn sample_board() -> Board {
    let mut board = Board::new();
    board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
    board.layers.push(Layer::copper(LayerId(1), "B.Cu"));

    let mut class = NetClass::default_placeholder();
    class.clearance_nm = 180_000;
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

    board.tracks.push(Track {
        id: TrackId(0),
        segment: Segment::new(Point::new(0, 0), Point::new(2_000_000, 1_000_000)),
        width_nm: 250_000,
        layer: LayerId(0),
        net: net_a,
        locked: false,
    });
    board.vias.push(Via {
        id: ViaId(0),
        position: Point::new(500_000, 500_000),
        diameter_nm: 600_000,
        drill_nm: 300_000,
        from_layer: LayerId(0),
        to_layer: LayerId(1),
        net: net_a,
        locked: true,
    });
    board.pads.push(Pad {
        id: PadId(0),
        shape: PadShape::Circle(Circle::new(Point::new(3_000_000, 0), 400_000)),
        layers: vec![LayerId(0)],
        net: net_b,
        locked: false,
        reference: None,
        number: None,
    });

    board
}

#[test]
fn write_then_parse_recovers_equivalent_board() {
    let original = sample_board();
    let (pcb_text, pro_text) = write_fixture(&original);

    let parsed = parse_board(&pcb_text, Some(&pro_text)).expect("parse fixture we just wrote");
    assert!(
        parsed.warnings.is_empty(),
        "unexpected warnings: {:?}",
        parsed.warnings
    );
    let board = parsed.board;

    assert_eq!(board.layers.len(), 2);
    assert_eq!(board.nets.len(), original.nets.len());
    assert_eq!(board.net_class_for(NetId(1)).unwrap().clearance_nm, 180_000);

    assert_eq!(board.tracks.len(), 1);
    let track = &board.tracks[0];
    assert_eq!(track.segment, original.tracks[0].segment);
    assert_eq!(track.width_nm, original.tracks[0].width_nm);
    assert_eq!(track.net, net_id(&original, "A"));

    assert_eq!(board.vias.len(), 1);
    let via = &board.vias[0];
    assert_eq!(via.position, original.vias[0].position);
    assert_eq!(via.diameter_nm, original.vias[0].diameter_nm);
    assert!(via.locked, "via lock state must round-trip");

    assert_eq!(board.pads.len(), 1);
    let PadShape::Circle(parsed_circle) = &board.pads[0].shape;
    let PadShape::Circle(original_circle) = &original.pads[0].shape;
    assert_eq!(parsed_circle, original_circle);
}

fn net_id(board: &Board, name: &str) -> NetId {
    *board
        .nets
        .values()
        .find(|n| n.name == name)
        .map(|n| &n.id)
        .expect("net exists")
}
