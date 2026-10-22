// Measuring how closely two routes run side by side is float geometry on
// nanometre coordinates far below f64's 52-bit mantissa.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use tessera_drc::check_clearance;
use tessera_engine::route_board;
use tessera_geom::{Circle, Point, Segment};
use tessera_model::{Board, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape, Track};

fn pad(id: u32, net: NetId, x: i64, y: i64) -> Pad {
    Pad {
        id: PadId(id),
        shape: PadShape::Circle(Circle::new(Point::new(x, y), 150_000)),
        layers: vec![LayerId(0)],
        net,
        locked: false,
        reference: None,
        number: None,
        drill_nm: None,
    }
}

fn distance_nm(p: (f64, f64), s: Segment) -> f64 {
    let (ax, ay) = (s.a.x as f64, s.a.y as f64);
    let (dx, dy) = (s.b.x as f64 - ax, s.b.y as f64 - ay);
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq == 0.0 {
        0.0
    } else {
        (((p.0 - ax) * dx + (p.1 - ay) * dy) / len_sq).clamp(0.0, 1.0)
    };
    (p.0 - ax - t * dx).hypot(p.1 - ay - t * dy)
}

/// The fraction of `tracks`' length lying within 60µm of `target_nm`
/// from the nearest of `partner`'s tracks.
fn coupled_fraction(tracks: &[&Track], partner: &[&Track], target_nm: f64) -> f64 {
    let (mut total, mut coupled) = (0.0, 0.0);
    for t in tracks {
        let (ax, ay) = (t.segment.a.x as f64, t.segment.a.y as f64);
        let (bx, by) = (t.segment.b.x as f64, t.segment.b.y as f64);
        let steps = ((bx - ax).hypot(by - ay) / 50_000.0).ceil().max(1.0) as usize;
        for k in 0..steps {
            let f = (k as f64 + 0.5) / steps as f64;
            let p = (ax + f * (bx - ax), ay + f * (by - ay));
            let d = partner
                .iter()
                .filter(|q| q.layer == t.layer)
                .map(|q| distance_nm(p, q.segment))
                .fold(f64::INFINITY, f64::min);
            total += 1.0;
            if (d - target_nm).abs() <= 60_000.0 {
                coupled += 1.0;
            }
        }
    }
    coupled / total
}

#[test]
fn a_diff_pair_is_routed_side_by_side_at_the_pair_gap() {
    let mut board = Board::new();
    board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
    board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
    let mut class = NetClass::default_placeholder();
    class.clearance_nm = 200_000;
    class.track_width_nm = 250_000;
    class.diff_pair_track_width_nm = Some(200_000);
    class.diff_pair_gap_nm = Some(250_000);
    board.net_classes.insert("Default".to_string(), class);
    for (id, name) in [(1, "USB_P"), (2, "USB_N")] {
        board.nets.insert(
            NetId(id),
            Net {
                id: NetId(id),
                name: name.to_string(),
                net_class: "Default".to_string(),
            },
        );
    }
    // Pads 0.65mm apart at both ends (the pair runs closer, at 0.45mm, in
    // between), the far end offset diagonally.
    board.pads.push(pad(0, NetId(1), 0, 0));
    board.pads.push(pad(1, NetId(2), 0, 650_000));
    board.pads.push(pad(2, NetId(1), 12_000_000, 6_000_000));
    board.pads.push(pad(3, NetId(2), 12_000_000, 6_650_000));

    let report = route_board(&mut board);
    assert_eq!(report.routed, 2, "{report:?}");
    assert!(check_clearance(&board).is_empty());

    let p: Vec<&Track> = board.tracks.iter().filter(|t| t.net == NetId(1)).collect();
    let n: Vec<&Track> = board.tracks.iter().filter(|t| t.net == NetId(2)).collect();
    // Pair width, and coupled at width + gap = 0.45mm for most of the way.
    assert!(p.iter().chain(&n).all(|t| t.width_nm == 200_000));
    for t in p.iter().chain(&n) {
        eprintln!(
            "{:?} {:?} ({},{})->({},{})",
            t.net, t.layer, t.segment.a.x, t.segment.a.y, t.segment.b.x, t.segment.b.y
        );
    }
    let fraction = coupled_fraction(&n, &p, 450_000.0);
    assert!(
        fraction >= 0.8,
        "only {:.0}% of the pair is coupled",
        fraction * 100.0
    );
}
