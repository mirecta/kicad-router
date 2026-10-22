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
        reference: None,
        number: None,
        drill_nm: None,
    });
    board.pads.push(Pad {
        id: PadId(1),
        shape: PadShape::Circle(Circle::new(Point::new(2_000_000, 0), 200_000)),
        layers: vec![LayerId(0)],
        net,
        locked: false,
        reference: None,
        number: None,
        drill_nm: None,
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

const FOUR_LAYER_BOARD: &str = r#"(kicad_pcb
	(version 20260206)
	(generator "pcbnew")
	(generator_version "10.0")
	(general (thickness 1.6))
	(paper "A4")
	(layers
		(0 "F.Cu" signal)
		(4 "In1.Cu" signal)
		(6 "In2.Cu" signal)
		(2 "B.Cu" signal)
		(25 "Edge.Cuts" user)
	)
	(setup (pad_to_mask_clearance 0))
	(footprint "t:U" (layer "F.Cu") (at 8 10)
		(pad "1" smd rect (at -1 0) (size 0.8 0.8) (layers "F.Cu") (net "SIG"))
		(pad "2" smd rect (at 1 0) (size 0.8 0.8) (layers "F.Cu") (net "GND"))
		(pad "3" smd rect (at 0 2) (size 0.8 0.8) (layers "F.Cu") (net "VCC"))
	)
	(footprint "t:J" (layer "F.Cu") (at 22 10)
		(pad "1" thru_hole circle (at 0 0) (size 1.7 1.7) (drill 1) (layers "*.Cu" "*.Mask") (net "SIG"))
		(pad "2" thru_hole circle (at 0 2.54) (size 1.7 1.7) (drill 1) (layers "*.Cu" "*.Mask") (net "GND"))
		(pad "3" thru_hole circle (at 0 -2.54) (size 1.7 1.7) (drill 1) (layers "*.Cu" "*.Mask") (net "VCC"))
	)
	(gr_rect (start 0 0) (end 30 20) (stroke (width 0.1) (type solid)) (fill no) (layer "Edge.Cuts"))
)
"#;

const FOUR_LAYER_PROJECT: &str = r#"{"net_settings": {"classes": [{"name": "Default", "clearance": 0.2, "track_width": 0.25, "via_diameter": 0.6, "via_drill": 0.3}], "meta": {"version": 4}}}"#;

/// `--plane` end to end, judged by real KiCad: signals on the outer layers,
/// the surface GND/VCC pads fanned out, two plane zones added — and after
/// KiCad refills those zones, nothing is left unconnected and no copper
/// rule is broken.
#[test]
fn route_with_planes_connects_everything_after_kicad_fills_the_planes() {
    let dir = std::env::temp_dir().join(format!("tessera-cli-e2e-planes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let input_pcb = dir.join("input.kicad_pcb");
    let output_pcb = dir.join("output.kicad_pcb");
    std::fs::write(&input_pcb, FOUR_LAYER_BOARD).expect("write input .kicad_pcb");
    std::fs::write(dir.join("input.kicad_pro"), FOUR_LAYER_PROJECT)
        .expect("write input .kicad_pro");

    let items_path = dir.join("new_items.json");
    let status = Command::new(env!("CARGO_BIN_EXE_tessera-cli"))
        .args(["route", "--plane", "In1.Cu=VCC", "--plane", "In2.Cu=GND"])
        .arg("--new-items")
        .arg(&items_path)
        .arg(&input_pcb)
        .arg(&output_pcb)
        .status()
        .expect("run tessera-cli route");
    assert!(status.success(), "tessera-cli route exited non-zero");

    // The JSON the KiCad plugin creates items from: every new track is on a
    // signal layer with a known net, and the two plane zones are there.
    let items: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&items_path).expect("read new items"))
            .expect("new items JSON");
    let tracks = items["tracks"].as_array().expect("tracks");
    assert!(!tracks.is_empty());
    assert!(tracks
        .iter()
        .all(|t| ["F.Cu", "B.Cu"].contains(&t["layer"].as_str().unwrap_or(""))));
    assert!(tracks
        .iter()
        .all(|t| ["SIG", "GND", "VCC"].contains(&t["net"].as_str().unwrap_or(""))));
    let mut zone_layers: Vec<&str> = items["zones"]
        .as_array()
        .expect("zones")
        .iter()
        .map(|z| z["layer"].as_str().unwrap_or(""))
        .collect();
    zone_layers.sort_unstable();
    assert_eq!(zone_layers, ["In1.Cu", "In2.Cu"]);
    assert_eq!(items["zones"][0]["thermal_gap"], 250_000);
    assert!(items["zones"][0]["outline"]
        .as_array()
        .is_some_and(|o| o.len() == 4));
    let routed = std::fs::read_to_string(&output_pcb).expect("read output");
    assert_eq!(routed.matches("(zone").count(), 2);
    assert!(
        !routed.contains("(layer \"In1.Cu\")\n\t\t(net")
            && !routed.contains("(layer \"In2.Cu\")\n\t\t(net")
    );

    if !kicad_cli_available() {
        eprintln!("kicad-cli not found on PATH; skipping real-DRC verification");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let drc_out = dir.join("drc.json");
    let drc_status = Command::new("kicad-cli")
        .args([
            "pcb",
            "drc",
            "--refill-zones",
            "--format",
            "json",
            "--severity-all",
            "-o",
        ])
        .arg(&drc_out)
        .arg(&output_pcb)
        .status()
        .expect("run kicad-cli pcb drc");
    assert!(drc_status.success());
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&drc_out).expect("read DRC report"))
            .expect("parse DRC report");
    let copper_problems: Vec<&serde_json::Value> = report["violations"]
        .as_array()
        .expect("violations array")
        .iter()
        .filter(|v| {
            [
                "clearance",
                "shorting_items",
                "hole_clearance",
                "hole_to_hole",
                "annular_width",
                "starved_thermal",
                "track_dangling",
                "via_dangling",
            ]
            .iter()
            .any(|t| v["type"] == *t)
        })
        .collect();
    assert!(copper_problems.is_empty(), "{copper_problems:?}");
    let unconnected = report["unconnected_items"].as_array().map_or(0, Vec::len);
    assert_eq!(unconnected, 0, "{}", report["unconnected_items"]);

    let _ = std::fs::remove_dir_all(&dir);
}
