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
use tessera_drc::check_clearance;
use tessera_geom::{Point, Segment};
use tessera_io_kicad::fixture::write_fixture;
use tessera_model::{Board, Layer, LayerId, Net, NetClass, NetId, Track, TrackId};

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
        .any(|v| v["type"] == "clearance");

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
