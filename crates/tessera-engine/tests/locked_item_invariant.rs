//! Plan §7.5.4's invariant, with its own dedicated test as required:
//! "a locked track, via, or footprint is an immovable obstacle and is
//! never a rip-up candidate." M2 has no rip-up scheduler yet (that's M5),
//! so there's nothing that could *rip up* a locked item — but this test
//! still establishes and checks the load-bearing half of the invariant
//! available today: routing must treat a locked item as a genuine,
//! unmovable obstacle (never routing through it even when that's the only
//! way to succeed) and must never mutate it while routing around it.
//!
//! Two scenarios, both built as an adversarial "rip-up trap": a locked
//! track forms a wall taller than the router's local search window, so
//! there's no way around it within that window short of changing layers.
//!
//! - `detour_exists`: the wall only blocks F.Cu. The router must find the
//!   B.Cu detour (via down, cross, via back up) rather than give up, and
//!   the locked wall must come out of the process byte-for-byte unchanged.
//! - `no_corridor_exists`: matching walls block both F.Cu and B.Cu, so no
//!   detour exists at all. The router must fail to route rather than
//!   somehow "succeed" by compromising the lock — and the locked walls
//!   must still be unchanged and still present.

use tessera_engine::route_board;
use tessera_geom::{Circle, Point, Segment};
use tessera_model::{
    Board, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape, Track, TrackId,
};

fn base_board() -> Board {
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

    let net_a = NetId(1);
    board.nets.insert(
        net_a,
        Net {
            id: net_a,
            name: "A".to_string(),
            net_class: "Default".to_string(),
        },
    );
    board.pads.push(Pad {
        id: PadId(0),
        shape: PadShape::Circle(Circle::new(Point::new(0, 0), 200_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
    });
    board.pads.push(Pad {
        id: PadId(1),
        shape: PadShape::Circle(Circle::new(Point::new(5_000_000, 0), 200_000)),
        layers: vec![LayerId(0)],
        net: net_a,
        locked: false,
    });

    board
}

fn add_locked_wall(board: &mut Board, id: u32, layer: LayerId) -> Track {
    // Taller than the router's local search window (origin/far each extend
    // SEARCH_MARGIN_NM = 3mm past the pads, so the window is at most ~6mm
    // tall here) — this wall cannot be routed around within the window,
    // only through a layer change.
    let track = Track {
        id: TrackId(id),
        segment: Segment::new(
            Point::new(2_500_000, -5_000_000),
            Point::new(2_500_000, 5_000_000),
        ),
        width_nm: 500_000,
        layer,
        net: NetId(999), // an unrelated locked net; never the one being routed
        locked: true,
    };
    board.tracks.push(track.clone());
    track
}

#[test]
fn detour_exists_locked_wall_forces_layer_change_and_is_unmodified() {
    let mut board = base_board();
    let wall = add_locked_wall(&mut board, 0, LayerId(0)); // F.Cu only

    let report = route_board(&mut board);

    assert_eq!(report.routed, 1, "should route around via B.Cu: {report:?}");
    assert!(report.failed.is_empty());

    // The router should have needed at least one via to get past the wall.
    assert!(
        !board.vias.is_empty(),
        "expected a layer change to route around the F.Cu-only wall"
    );

    let surviving = board
        .tracks
        .iter()
        .find(|t| t.id == wall.id)
        .expect("locked wall must still be present");
    assert_eq!(
        *surviving, wall,
        "locked item must be byte-for-byte unchanged after routing"
    );
}

#[test]
fn no_corridor_exists_router_fails_rather_than_compromise_locks() {
    let mut board = base_board();
    let wall_f = add_locked_wall(&mut board, 0, LayerId(0));
    let wall_b = add_locked_wall(&mut board, 1, LayerId(1));

    let report = route_board(&mut board);

    assert_eq!(
        report.routed, 0,
        "no path exists on either layer; must not falsely succeed: {report:?}"
    );
    assert_eq!(report.failed, vec![NetId(1)]);

    // Both locked walls must survive completely untouched — no rip-up, no
    // mutation, not even as a side effect of a failed search.
    for wall in [&wall_f, &wall_b] {
        let surviving = board
            .tracks
            .iter()
            .find(|t| t.id == wall.id)
            .expect("locked wall must still be present");
        assert_eq!(*surviving, *wall, "locked item must be unchanged");
    }

    // The net must still be reported as needing a route — routing failure
    // must be visible, not silently swallowed.
    let after = board.find_unrouted_connections();
    assert_eq!(after.connections.len(), 1);
    assert_eq!(after.connections[0].net, NetId(1));
}
