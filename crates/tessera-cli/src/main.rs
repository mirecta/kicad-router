#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use tessera_model::Plane;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("route") {
        route_command(&args[2..])
    } else {
        eprintln!(
            "Usage: tessera-cli route [--plane LAYER=NET]... <input.kicad_pcb> <output.kicad_pcb>"
        );
        std::process::exit(2);
    }
}

/// `tessera-cli route <input.kicad_pcb> <output.kicad_pcb>`: ingest,
/// route every unrouted net, commit, write out. The output is your
/// *original* file's own text with only the newly-routed tracks/vias
/// spliced in (`tessera_io_kicad::commit::splice_new_geometry`) — copper
/// pours, board outline, silkscreen, everything else survives byte-for-
/// byte untouched, since routing never modifies anything except adding
/// new copper for previously-unrouted nets. A companion
/// `<input-stem>.kicad_pro` is read if present (for net-class clearance/
/// width, per ADR-0002) and copied unchanged to the output path — this
/// tool never modifies project settings, so there's nothing to
/// regenerate.
///
/// `--plane LAYER=NET` (repeatable, e.g. `--plane In1.Cu=+3.3V --plane
/// In2.Cu=GND`) reserves a copper layer as that net's plane: no signal is
/// routed on it, the net's surface pads get a short fanout via into it, and
/// if no zone of that net covers the layer yet an unfilled board-sized one
/// is added (fill it in KiCad with B).
fn route_command(args: &[String]) -> Result<()> {
    let (input, output, plane_args) = parse_route_args(args)?;
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
    add_planes(&mut board, &plane_args)?;
    let original_track_count = board.tracks.len();
    let original_via_count = board.vias.len();

    let report = tessera_engine::route_board(&mut board);
    eprintln!(
        "routed {} net(s); {} failed; {} skipped",
        report.routed,
        report.failed.len(),
        report.skipped.len()
    );
    if !board.planes.is_empty() {
        eprintln!(
            "plane fanouts: {} pad(s) connected; {} could not be",
            report.fanouts,
            report.fanout_failed.len()
        );
    }
    for note in &report.skipped {
        eprintln!("skipped: {note}");
    }
    if !report.failed.is_empty() {
        eprintln!("failed to route nets: {:?}", report.failed);
    }

    // route_board only ever pushes newly-routed items to the end of
    // these Vecs, never reorders or removes existing ones — see
    // tessera_engine::route_board's own docs — so everything from the
    // pre-routing length onward is exactly what this pass added.
    let new_tracks = &board.tracks[original_track_count..];
    let new_vias = &board.vias[original_via_count..];
    let new_planes: Vec<Plane> = board
        .planes
        .iter()
        .filter(|p| {
            !board
                .copper_zones
                .iter()
                .any(|z| z.net == p.net && z.layers.contains(&p.layer))
        })
        .copied()
        .collect();
    if !new_planes.is_empty() && board.outline.is_none() {
        bail!("can't add plane zones: the board outline couldn't be read");
    }

    let out_pcb = tessera_io_kicad::commit::splice_new_geometry(
        &pcb_text,
        &board,
        new_tracks,
        new_vias,
        &new_planes,
    )
    .context("could not splice new routing into the input file's own text")?;
    if !new_planes.is_empty() {
        eprintln!(
            "added {} unfilled plane zone(s) — fill them in KiCad (B)",
            new_planes.len()
        );
    }
    fs::write(&output_path, out_pcb)
        .with_context(|| format!("writing {}", output_path.display()))?;

    let out_pro_path = output_path.with_extension("kicad_pro");
    if let Some(pro_text) = &pro_text {
        fs::write(&out_pro_path, pro_text)
            .with_context(|| format!("writing {}", out_pro_path.display()))?;
        println!(
            "wrote {} and {}",
            output_path.display(),
            out_pro_path.display()
        );
    } else {
        println!(
            "wrote {} (no input .kicad_pro found, so none was written)",
            output_path.display()
        );
    }
    Ok(())
}

/// A `--plane LAYER=NET` argument, as names.
type PlaneArg = (String, String);

/// `[--plane LAYER=NET]... <input> <output>` → (input, output, planes).
fn parse_route_args(args: &[String]) -> Result<(&String, &String, Vec<PlaneArg>)> {
    let mut plane_args = Vec::new();
    let mut positional = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--plane" {
            let spec = rest.next().context("--plane needs LAYER=NET")?;
            let (layer, net) = spec
                .split_once('=')
                .with_context(|| format!("--plane {spec}: expected LAYER=NET"))?;
            plane_args.push((layer.to_string(), net.to_string()));
        } else {
            positional.push(arg);
        }
    }
    let [input, output] = positional[..] else {
        bail!("route requires exactly two file arguments: <input.kicad_pcb> <output.kicad_pcb>");
    };
    Ok((input, output, plane_args))
}

fn add_planes(board: &mut tessera_model::Board, plane_args: &[PlaneArg]) -> Result<()> {
    for (layer_name, net_name) in plane_args {
        let layer = board
            .layers
            .iter()
            .find(|l| &l.name == layer_name)
            .with_context(|| format!("--plane: no copper layer named {layer_name}"))?
            .id;
        let net = board
            .nets
            .values()
            .find(|n| &n.name == net_name)
            .with_context(|| format!("--plane: no net named {net_name}"))?
            .id;
        board.planes.push(Plane { layer, net });
    }
    Ok(())
}
