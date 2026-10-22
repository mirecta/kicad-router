//! The M1 DRC parity harness (plan §3.2): generate candidate geometry,
//! ask `tessera-drc` and KiCad's own `kicad-cli pcb drc` the same question
//! — "any clearance violation here?" — and fail on disagreement.
//!
//! This test dev-depends on `tessera-drc` from `tessera-io-kicad`, which
//! looks like it runs against the plan §2.2 dependency rule ("`io-*` depend
//! on `model` only"). It doesn't: that rule governs the crates' *production*
//! dependency graph (what ships), and Cargo keeps `[dev-dependencies]`
//! entirely separate from it — no downstream consumer of `tessera-io-kicad`
//! as a library ever sees this edge. A no-back-edges CI check should assert
//! against each crate's normal dependencies, not its dev-dependencies.
//!
//! Requires `kicad-cli` on `PATH` (confirmed present in this environment,
//! KiCad 10.0.3). Skips with a clear message rather than failing if it's
//! absent, since we don't yet know whether every environment this runs in
//! will have KiCad installed — tighten to a hard failure once a
//! KiCad-equipped CI runner is confirmed (plan §9.2 wants this "blocking,
//! every commit").

use std::process::Command;

use proptest::prelude::*;
use tessera_drc::{
    check_clearance, check_edge_clearance, check_hole_clearance, check_hole_to_hole,
    check_keepouts, check_via_annular_width, check_zone_clearance,
};
use tessera_geom::{Circle, Point, Polygon, Segment};
use tessera_io_kicad::fixture::write_fixture;
use tessera_model::{
    Board, Hole, HoleId, KeepoutFlags, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape,
    RuleArea, RuleAreaId, Track, TrackId, Via, ViaId, ZoneFill,
};

fn kicad_cli_available() -> bool {
    Command::new("kicad-cli")
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn two_track_board(offset_nm: i64, width_nm: i64, clearance_nm: i64) -> Board {
    let mut board = Board::new();
    board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
    board.layers.push(Layer::copper(LayerId(1), "B.Cu"));

    let mut class = NetClass::default_placeholder();
    class.clearance_nm = clearance_nm;
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
        segment: Segment::new(Point::new(0, 0), Point::new(2_000_000, 0)),
        width_nm,
        layer: LayerId(0),
        net: net_a,
        locked: false,
    });
    board.tracks.push(Track {
        id: TrackId(1),
        segment: Segment::new(Point::new(0, offset_nm), Point::new(2_000_000, offset_nm)),
        width_nm,
        layer: LayerId(0),
        net: net_b,
        locked: false,
    });

    board
}

/// Runs `kicad-cli pcb drc` on `board` and returns whether it reported any
/// `"clearance"`-type violation. Other violation types (dangling tracks,
/// missing board outline — expected artefacts of a minimal synthetic
/// fixture, not what this harness checks) are ignored.
fn kicad_reports_clearance_violation(board: &Board) -> bool {
    kicad_reports_violation(board, "clearance")
}

/// Runs `kicad-cli pcb drc` on `board` and returns whether it reported any
/// violation of type `violation_type`.
fn kicad_reports_violation(board: &Board, violation_type: &str) -> bool {
    let (pcb_text, pro_text) = write_fixture(board);

    let dir = std::env::temp_dir().join(format!(
        "tessera-drc-parity-{}-{}",
        std::process::id(),
        fastrand_like_counter()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let pcb_path = dir.join("fixture.kicad_pcb");
    let pro_path = dir.join("fixture.kicad_pro");
    let out_path = dir.join("drc.json");
    std::fs::write(&pcb_path, pcb_text).expect("write .kicad_pcb");
    std::fs::write(&pro_path, pro_text).expect("write .kicad_pro");

    let status = Command::new("kicad-cli")
        .args(["pcb", "drc", "--format", "json", "--severity-all", "-o"])
        .arg(&out_path)
        .arg(&pcb_path)
        .status()
        .expect("run kicad-cli");
    assert!(status.success(), "kicad-cli exited non-zero");

    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out_path).expect("read DRC report"))
            .expect("parse DRC report as JSON");

    let has_clearance_violation = report["violations"]
        .as_array()
        .expect("violations array")
        .iter()
        .any(|v| v["type"] == violation_type);

    let _ = std::fs::remove_dir_all(&dir);
    has_clearance_violation
}

// A plain incrementing counter is enough to avoid two concurrent proptest
// cases colliding on the same temp directory; true randomness isn't needed.
fn fastrand_like_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, .. ProptestConfig::default() })]

    /// Parametrised directly by the true edge-to-edge gap (not raw
    /// centerline offset), so every case is a clean "gap vs. required
    /// clearance" comparison. **Deliberately excludes overlap** (gap < 0):
    /// an earlier version of this harness parametrised by offset instead
    /// and immediately found that KiCad's own `kicad-cli pcb drc` reports
    /// *no* `"clearance"` violation at all — not even a different
    /// violation type — when two different-net tracks' copper fully
    /// overlaps (verified directly: a 0.05mm actual gap correctly produces
    /// a `clearance` violation; changing only the offset so the same
    /// tracks instead overlap by 0.05mm produces zero violations of any
    /// kind). That's a genuine, surprising KiCad DRC gap, not a bug in this
    /// harness or in `tessera-drc` — recorded in `docs/DRC_PARITY.md` as
    /// `DEGRADED` rather than silently worked around. This test only
    /// covers the gap >= 0 region until that entry is resolved.
    #[test]
    fn track_track_clearance_matches_kicad(
        gap_nm in 0i64..600_000,
        width_nm in 100_000i64..300_000,
        clearance_nm in 100_000i64..300_000,
    ) {
        if !kicad_cli_available() {
            eprintln!("kicad-cli not found on PATH; skipping DRC parity check");
            return Ok(());
        }

        let offset_nm = width_nm + gap_nm;
        let board = two_track_board(offset_nm, width_nm, clearance_nm);
        let ours_has_violation = !check_clearance(&board).is_empty();
        let kicad_has_violation = kicad_reports_clearance_violation(&board);

        prop_assert_eq!(
            ours_has_violation,
            kicad_has_violation,
            "tessera-drc={}, kicad-cli={} (gap={}nm, width={}nm, clearance={}nm)",
            ours_has_violation, kicad_has_violation, gap_nm, width_nm, clearance_nm
        );
    }
}

/// A through-hole pad (copper on both layers) and a track on **B.Cu** —
/// the layer a writer that only emitted a pad's first layer would have
/// silently left empty, letting both sides agree on "no violation" for the
/// wrong reason. Cases straddle the clearance threshold on both sides.
#[test]
fn through_hole_pad_clearance_on_the_back_layer_matches_kicad() {
    if !kicad_cli_available() {
        eprintln!("kicad-cli not found on PATH; skipping DRC parity check");
        return;
    }
    let clearance_nm = 200_000;
    for gap_nm in [50_000, 199_000, 201_000, 400_000] {
        let mut board = two_track_board(0, 200_000, clearance_nm);
        board.tracks.clear();
        let pad_radius_nm = 800_000;
        board.pads.push(Pad {
            id: PadId(0),
            shape: PadShape::Circle(Circle::new(Point::new(0, 0), pad_radius_nm)),
            layers: vec![LayerId(0), LayerId(1)],
            net: NetId(1),
            locked: false,
            reference: None,
            number: None,
            drill_nm: Some(800_000),
        });
        let y = pad_radius_nm + 100_000 + gap_nm;
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(-2_000_000, y), Point::new(2_000_000, y)),
            width_nm: 200_000,
            layer: LayerId(1),
            net: NetId(2),
            locked: false,
        });
        let ours = !check_clearance(&board).is_empty();
        let kicad = kicad_reports_clearance_violation(&board);
        assert_eq!(
            ours, kicad,
            "tessera-drc={ours}, kicad-cli={kicad} (gap={gap_nm}nm)"
        );
        assert_eq!(ours, gap_nm < clearance_nm, "gap={gap_nm}nm");
    }
}

/// KiCad tolerates a gap up to 500nm short of the required clearance; the
/// random-gap proptest above almost never lands inside that window, so the
/// edge is pinned down explicitly here.
#[test]
fn clearance_tolerance_edge_matches_kicad() {
    if !kicad_cli_available() {
        eprintln!("kicad-cli not found on PATH; skipping DRC parity check");
        return;
    }
    let width_nm = 200_000;
    let clearance_nm = 200_000;
    for short_by_nm in [499, 500, 501, 1_000] {
        let gap_nm = clearance_nm - short_by_nm;
        let board = two_track_board(width_nm + gap_nm, width_nm, clearance_nm);
        let ours = !check_clearance(&board).is_empty();
        let kicad = kicad_reports_clearance_violation(&board);
        assert_eq!(
            ours, kicad,
            "tessera-drc={ours}, kicad-cli={kicad} (short by {short_by_nm}nm)"
        );
        assert_eq!(ours, short_by_nm > 500, "short by {short_by_nm}nm");
    }
}

/// A non-plated hole is held to the board's `min_hole_clearance` (here a
/// non-default 0.3mm, so the project file must carry it), not net-class
/// clearance, on every layer, with the same 500nm tolerance.
#[test]
fn npth_hole_clearance_matches_kicad() {
    if !kicad_cli_available() {
        eprintln!("kicad-cli not found on PATH; skipping DRC parity check");
        return;
    }
    let required_nm = 300_000;
    for gap_nm in [100_000, 299_499, 299_501, 400_000] {
        let mut board = two_track_board(0, 200_000, 200_000);
        board.tracks.clear();
        board.rules.min_hole_clearance_nm = required_nm;
        board.npth_holes.push(Hole {
            id: HoleId(0),
            position: Point::new(0, 0),
            diameter_nm: 2_000_000,
        });
        let x = 1_000_000 + gap_nm + 100_000;
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(x, -2_000_000), Point::new(x, 2_000_000)),
            width_nm: 200_000,
            layer: LayerId(1),
            net: NetId(1),
            locked: false,
        });
        let ours = !check_hole_clearance(&board).is_empty();
        let kicad = kicad_reports_violation(&board, "hole_clearance");
        assert_eq!(
            ours, kicad,
            "tessera-drc={ours}, kicad-cli={kicad} (gap={gap_nm}nm)"
        );
        assert_eq!(ours, gap_nm < required_nm - 500, "gap={gap_nm}nm");
    }
}

/// The annular-ring check is exact (no 500nm tolerance): 1nm too small a
/// diameter is already a violation.
#[test]
fn via_annular_width_matches_kicad() {
    if !kicad_cli_available() {
        eprintln!("kicad-cli not found on PATH; skipping DRC parity check");
        return;
    }
    for diameter_nm in [700_000, 699_999, 690_000] {
        let mut board = two_track_board(0, 200_000, 200_000);
        board.tracks.clear();
        board.rules.min_via_annular_width_nm = 200_000;
        board.vias.push(Via {
            id: ViaId(0),
            position: Point::new(0, 0),
            diameter_nm,
            drill_nm: 300_000,
            from_layer: LayerId(0),
            to_layer: LayerId(1),
            net: NetId(1),
            locked: false,
        });
        let ours = !check_via_annular_width(&board).is_empty();
        let kicad = kicad_reports_violation(&board, "annular_width");
        assert_eq!(
            ours, kicad,
            "tessera-drc={ours}, kicad-cli={kicad} (diameter={diameter_nm}nm)"
        );
        assert_eq!(ours, diameter_nm < 700_000, "diameter={diameter_nm}nm");
    }
}

/// `hole_to_hole` between two vias of the *same* net, straddling the
/// 500nm tolerance.
#[test]
fn same_net_hole_to_hole_matches_kicad() {
    if !kicad_cli_available() {
        eprintln!("kicad-cli not found on PATH; skipping DRC parity check");
        return;
    }
    for short_by_nm in [499, 501, 100_000] {
        let mut board = two_track_board(0, 200_000, 200_000);
        board.tracks.clear();
        board.rules.min_via_annular_width_nm = 0;
        let gap_nm = board.rules.min_hole_to_hole_nm - short_by_nm;
        for (id, x) in [(0, 0), (1, 300_000 + gap_nm)] {
            board.vias.push(Via {
                id: ViaId(id),
                position: Point::new(x, 0),
                diameter_nm: 400_000,
                drill_nm: 300_000,
                from_layer: LayerId(0),
                to_layer: LayerId(1),
                net: NetId(1),
                locked: false,
            });
        }
        let ours = !check_hole_to_hole(&board).is_empty();
        let kicad = kicad_reports_violation(&board, "hole_to_hole");
        assert_eq!(
            ours, kicad,
            "tessera-drc={ours}, kicad-cli={kicad} (short by {short_by_nm}nm)"
        );
        assert_eq!(ours, short_by_nm > 500, "short by {short_by_nm}nm");
    }
}

/// A track beside another net's zone fill: the larger of the zone's own
/// and the class clearance applies, with the 500nm tolerance.
#[test]
fn zone_fill_clearance_matches_kicad() {
    if !kicad_cli_available() {
        eprintln!("kicad-cli not found on PATH; skipping DRC parity check");
        return;
    }
    // (zone clearance, inflation, gap): class clearance is 0.2mm.
    for (zone_nm, inflation_nm, gap_nm) in [
        (250_000, 0, 249_501),
        (250_000, 0, 249_499),
        (250_000, 0, 220_000),
        (100_000, 0, 150_000),
        (100_000, 0, 210_000),
        (250_000, 125_000, 374_499),
        (250_000, 125_000, 374_501),
    ] {
        let mut board = two_track_board(0, 200_000, 200_000);
        board.tracks.truncate(1);
        let x = 5_000_000 + gap_nm + 100_000;
        board.tracks[0].segment = Segment::new(Point::new(x, 1_000_000), Point::new(x, 4_000_000));
        board.zone_fills.push(ZoneFill {
            net: NetId(2),
            layer: LayerId(0),
            clearance_nm: zone_nm,
            inflation_nm,
            polygon: Polygon::new(vec![
                Point::new(0, 0),
                Point::new(5_000_000, 0),
                Point::new(5_000_000, 5_000_000),
                Point::new(0, 5_000_000),
            ]),
        });
        let ours = !check_zone_clearance(&board).is_empty();
        let kicad = kicad_reports_clearance_violation(&board);
        assert_eq!(
            ours, kicad,
            "tessera-drc={ours}, kicad-cli={kicad} (zone={zone_nm}, inflation={inflation_nm}, gap={gap_nm})"
        );
    }
}

fn square(lo: i64, hi: i64) -> Polygon {
    Polygon::new(vec![
        Point::new(lo, lo),
        Point::new(hi, lo),
        Point::new(hi, hi),
        Point::new(lo, hi),
    ])
}

fn vertical_track(x: i64) -> Track {
    Track {
        id: TrackId(0),
        segment: Segment::new(Point::new(x, 5_000_000), Point::new(x, 15_000_000)),
        width_nm: 200_000,
        layer: LayerId(0),
        net: NetId(1),
        locked: false,
    }
}

/// Board-edge clearance (a non-default 0.4mm rule, so the project file
/// must carry it), straddling the 500nm tolerance, plus a track crossing
/// the edge.
#[test]
fn edge_clearance_matches_kicad() {
    if !kicad_cli_available() {
        eprintln!("kicad-cli not found on PATH; skipping DRC parity check");
        return;
    }
    let required_nm = 400_000;
    for gap_nm in [200_000, 399_499, 399_501, 600_000, -300_000] {
        let mut board = two_track_board(0, 200_000, 200_000);
        board.tracks.clear();
        board.outline = Some(square(0, 20_000_000));
        board.rules.min_copper_edge_clearance_nm = required_nm;
        board
            .tracks
            .push(vertical_track(20_000_000 - 100_000 - gap_nm));
        let ours = !check_edge_clearance(&board).is_empty();
        let kicad = kicad_reports_violation(&board, "copper_edge_clearance");
        assert_eq!(
            ours, kicad,
            "tessera-drc={ours}, kicad-cli={kicad} (gap={gap_nm}nm)"
        );
    }
}

/// A keepout forbidding tracks and vias on F.Cu: overlap counts, nearness
/// doesn't, other layers are free.
#[test]
fn keepout_overlap_matches_kicad() {
    if !kicad_cli_available() {
        eprintln!("kicad-cli not found on PATH; skipping DRC parity check");
        return;
    }
    let cases: [(&str, Option<Track>, Option<Via>); 5] = [
        ("crossing track", Some(vertical_track(10_000_000)), None),
        ("track 0.05mm off", Some(vertical_track(12_150_000)), None),
        (
            "track copper overlapping",
            Some(vertical_track(12_050_000)),
            None,
        ),
        (
            "track on B.Cu",
            Some(Track {
                layer: LayerId(1),
                ..vertical_track(10_000_000)
            }),
            None,
        ),
        (
            "via inside",
            None,
            Some(Via {
                id: ViaId(0),
                position: Point::new(10_000_000, 10_000_000),
                diameter_nm: 800_000,
                drill_nm: 400_000,
                from_layer: LayerId(0),
                to_layer: LayerId(1),
                net: NetId(1),
                locked: false,
            }),
        ),
    ];
    for (name, track, via) in cases {
        let mut board = two_track_board(0, 200_000, 200_000);
        board.tracks.clear();
        board.tracks.extend(track);
        board.vias.extend(via);
        board.rule_areas.push(RuleArea {
            id: RuleAreaId(0),
            name: "KO".to_string(),
            outline: square(8_000_000, 12_000_000),
            layers: vec![LayerId(0)],
            keepout: KeepoutFlags {
                tracks_allowed: false,
                vias_allowed: false,
                pads_allowed: true,
                copper_pour_allowed: true,
                footprints_allowed: true,
            },
        });
        let ours = !check_keepouts(&board).is_empty();
        let kicad = kicad_reports_violation(&board, "items_not_allowed");
        assert_eq!(ours, kicad, "{name}: tessera-drc={ours}, kicad-cli={kicad}");
    }
}

/// A net in a class with a larger clearance: the pair is held to the
/// larger of the two classes' clearances, in KiCad and in tessera alike.
#[test]
fn net_class_clearance_matches_kicad() {
    if !kicad_cli_available() {
        eprintln!("kicad-cli not found on PATH; skipping DRC parity check");
        return;
    }
    for gap_nm in [300_000, 499_501, 600_000] {
        let mut board = two_track_board(200_000 + gap_nm, 200_000, 200_000);
        let mut power = NetClass::default_placeholder();
        power.name = "Power".to_string();
        power.clearance_nm = 500_000;
        board.net_classes.insert("Power".to_string(), power);
        board.nets.get_mut(&NetId(2)).unwrap().net_class = "Power".to_string();
        let ours = !check_clearance(&board).is_empty();
        let kicad = kicad_reports_clearance_violation(&board);
        assert_eq!(
            ours, kicad,
            "tessera-drc={ours}, kicad-cli={kicad} (gap={gap_nm}nm)"
        );
        assert_eq!(ours, gap_nm < 500_000 - 500, "gap={gap_nm}nm");
    }
}
