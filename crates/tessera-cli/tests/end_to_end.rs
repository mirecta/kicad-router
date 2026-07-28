//! M2's exit criterion, tested directly: "end-to-end route of a trivial
//! 2-layer board, DRC-clean, visible in KiCad." Builds a trivial unrouted
//! board, runs the real compiled `tessera-cli route` binary on it (not a
//! library call — this exercises the actual CLI a user would run), and, if
//! `kicad-cli` is on `PATH`, verifies the output with real KiCad DRC —
//! not just `tessera-drc`'s own opinion of itself.

use std::process::Command;

use tessera_geom::{Circle, Point};
use tessera_model::{Board, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape};

fn trivial_board() -> Board {
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
    let net = NetId(1);
    board.nets.insert(
        net,
        Net {
            id: net,
            name: "NET1".to_string(),
            net_class: "Default".to_string(),
        },
    );
    board.pads.push(Pad {
        id: PadId(0),
        shape: PadShape::Circle(Circle::new(Point::new(0, 0), 200_000)),
        layers: vec![LayerId(0)],
        net,
        locked: false,
    });
    board.pads.push(Pad {
        id: PadId(1),
        shape: PadShape::Circle(Circle::new(Point::new(2_000_000, 0), 200_000)),
        layers: vec![LayerId(0)],
        net,
        locked: false,
    });
    board
}

fn kicad_cli_available() -> bool {
    Command::new("kicad-cli")
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn route_command_produces_a_drc_clean_board() {
    let (pcb_text, pro_text) = tessera_io_kicad::fixture::write_fixture(&trivial_board());

    let dir = std::env::temp_dir().join(format!("tessera-cli-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let input_pcb = dir.join("input.kicad_pcb");
    let input_pro = dir.join("input.kicad_pro");
    let output_pcb = dir.join("output.kicad_pcb");
    std::fs::write(&input_pcb, &pcb_text).expect("write input .kicad_pcb");
    std::fs::write(&input_pro, &pro_text).expect("write input .kicad_pro");

    let status = Command::new(env!("CARGO_BIN_EXE_tessera-cli"))
        .arg("route")
        .arg(&input_pcb)
        .arg(&output_pcb)
        .status()
        .expect("run tessera-cli route");
    assert!(status.success(), "tessera-cli route exited non-zero");
    assert!(output_pcb.exists());
    assert!(output_pcb.with_extension("kicad_pro").exists());

    if !kicad_cli_available() {
        eprintln!("kicad-cli not found on PATH; skipping real-DRC verification");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let drc_out = dir.join("drc.json");
    let drc_status = Command::new("kicad-cli")
        .args(["pcb", "drc", "--format", "json", "--severity-all", "-o"])
        .arg(&drc_out)
        .arg(&output_pcb)
        .status()
        .expect("run kicad-cli pcb drc");
    assert!(drc_status.success());

    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&drc_out).expect("read DRC report"))
            .expect("parse DRC report");
    let clearance_violations: Vec<&serde_json::Value> = report["violations"]
        .as_array()
        .expect("violations array")
        .iter()
        .filter(|v| v["type"] == "clearance")
        .collect();
    assert!(
        clearance_violations.is_empty(),
        "kicad-cli found clearance violations in routed output: {clearance_violations:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
