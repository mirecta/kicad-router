#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("route") {
        route_command(&args[2..])
    } else {
        eprintln!("Usage: tessera-cli route <input.kicad_pcb> <output.kicad_pcb>");
        std::process::exit(2);
    }
}

/// `tessera-cli route <input.kicad_pcb> <output.kicad_pcb>`: the M2
/// end-to-end demo — ingest, route every unrouted two-pin net, commit,
/// write out. A companion `<input-stem>.kicad_pro` is read if present (for
/// net-class clearance/width, per ADR-0002); the output always gets a
/// fresh companion `.kicad_pro` written alongside it, since
/// `tessera-io-kicad`'s current parser/writer pair only round-trips a
/// 2-layer-board subset of what a real project file carries (see
/// `fixture`/`parser`'s scope notes) — this is not yet the "preserve
/// everything, add only what changed" commit plan §2.3 describes for the
/// eventual production writer.
fn route_command(args: &[String]) -> Result<()> {
    let [input, output] = args else {
        bail!("route requires exactly two arguments: <input.kicad_pcb> <output.kicad_pcb>");
    };
    let input_path = PathBuf::from(input);
    let output_path = PathBuf::from(output);

    let pcb_text = fs::read_to_string(&input_path)
        .with_context(|| format!("reading {}", input_path.display()))?;
    let pro_path = input_path.with_extension("kicad_pro");
    let pro_text = fs::read_to_string(&pro_path).ok();

    let parsed = tessera_io_kicad::parser::parse_board(&pcb_text, pro_text.as_deref())
        .with_context(|| format!("parsing {}", input_path.display()))?;
    for warning in &parsed.warnings {
        eprintln!("warning: {warning}");
    }

    let mut board = parsed.board;
    let report = tessera_engine::route_board(&mut board);
    eprintln!(
        "routed {} net(s); {} failed; {} skipped",
        report.routed,
        report.failed.len(),
        report.skipped.len()
    );
    for note in &report.skipped {
        eprintln!("skipped: {note}");
    }
    if !report.failed.is_empty() {
        eprintln!("failed to route nets: {:?}", report.failed);
    }

    let (out_pcb, out_pro) = tessera_io_kicad::fixture::write_fixture(&board);
    fs::write(&output_path, out_pcb)
        .with_context(|| format!("writing {}", output_path.display()))?;
    let out_pro_path = output_path.with_extension("kicad_pro");
    fs::write(&out_pro_path, out_pro)
        .with_context(|| format!("writing {}", out_pro_path.display()))?;

    println!(
        "wrote {} and {}",
        output_path.display(),
        out_pro_path.display()
    );
    Ok(())
}
