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
            "Usage: tessera-cli route [--plane LAYER=NET]... [--no-pours] [--track-net NET]... [--soft-fill NET]... [--reroutable-from ORIGINAL.kicad_pcb] [--new-items FILE.json] <input.kicad_pcb> <output.kicad_pcb>"
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
///
/// A net with a copper zone (pour) is left to it, the way a designer
/// pours GND: pads on the pour layer connect through their thermals, the
/// rest get a fanout via into it, and no tracks are routed for it. KiCad
/// must refill the zones afterwards; if a pour ends up split into islands
/// by the new routing, rerun with `--track-net NET` to route that net as
/// tracks instead, or `--no-pours` for every net.
///
/// `--soft-fill NET` (repeatable) says that net's zone fills are only a
/// provisional refill of a pour this routing started (zones unfilled in
/// the user's board), to be refilled again after routing: they show which
/// pads the pour already joins but don't block new copper. Route, refill,
/// route again with the pour nets soft joins pour islands the first pass
/// left. Fills of other nets — the user's own finished pours — always
/// block.
///
/// `--reroutable-from ORIGINAL.kicad_pcb`, for such a later pass: every
/// track and via of the input that isn't in the original is the router's
/// own earlier output, and rip-up may take it out and route its net again
/// (it's removed from the written file). The original's copper is never
/// touched.
fn route_command(args: &[String]) -> Result<()> {
    let RouteArgs {
        input,
        output,
        planes: plane_args,
        no_pours,
        track_nets,
        soft_fills,
        new_items,
        reroutable_from,
    } = parse_route_args(args)?;
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
    set_soft_fills(&mut board, &soft_fills)?;
    if !no_pours {
        set_pour_nets(&mut board, &track_nets)?;
    }
    if let Some(base) = reroutable_from {
        mark_reroutable(&mut board, base, pro_text.as_deref())?;
    }
    let original_tracks = board.tracks.clone();
    let original_vias = board.vias.clone();

    let report = tessera_engine::route_board(&mut board);
    print_report(&board, &report);

    let changes = changes(&original_tracks, &original_vias, &board);
    let pcb_text = rip_out(&pcb_text, &board, &changes)?;
    let Changes {
        new_tracks,
        new_vias,
        removed_tracks,
        removed_vias,
    } = changes;
    let (new_tracks, new_vias) = (&new_tracks[..], &new_vias[..]);
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
    if let Some(path) = new_items {
        let json = new_items_json(
            &board,
            (new_tracks, new_vias),
            (&removed_tracks, &removed_vias),
            &new_planes,
        )?;
        fs::write(path, json).with_context(|| format!("writing {path}"))?;
    }

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

fn print_report(board: &tessera_model::Board, report: &tessera_engine::RouteReport) {
    eprintln!(
        "routed {} net(s); {} failed; {} skipped",
        report.routed,
        report.failed.len(),
        report.skipped.len()
    );
    if !board.pour_nets.is_empty() {
        let names: Vec<&str> = board
            .pour_nets
            .iter()
            .filter_map(|n| board.nets.get(n).map(|n| n.name.as_str()))
            .collect();
        eprintln!("left to their copper pours: {}", names.join(", "));
    }
    if !board.planes.is_empty() || !board.pour_nets.is_empty() {
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
}

/// What a pass added, and what of the input's (reroutable) copper rip-up
/// took out.
struct Changes {
    new_tracks: Vec<tessera_model::Track>,
    new_vias: Vec<tessera_model::Via>,
    removed_tracks: Vec<tessera_model::Track>,
    removed_vias: Vec<tessera_model::Via>,
}

/// `pcb_text` without the copper rip-up took out of it.
fn rip_out(pcb_text: &str, board: &tessera_model::Board, changes: &Changes) -> Result<String> {
    let (text, removed) = tessera_io_kicad::commit::remove_geometry(
        pcb_text,
        board,
        &changes.removed_tracks,
        &changes.removed_vias,
    );
    let expected = changes.removed_tracks.len() + changes.removed_vias.len();
    if removed != expected {
        bail!("rip-up removed {expected} item(s) but only {removed} were found in the file");
    }
    if removed > 0 {
        eprintln!("ripped up and rerouted {removed} item(s) of earlier passes");
    }
    Ok(text)
}

fn set_soft_fills(board: &mut tessera_model::Board, names: &[&String]) -> Result<()> {
    for name in names {
        let net = board
            .nets
            .values()
            .find(|n| &&n.name == name)
            .with_context(|| format!("--soft-fill: no net named {name}"))?
            .id;
        board.soft_fill_nets.push(net);
    }
    Ok(())
}

/// [`Changes`] from the input's items to `board`'s — by id: ids are never
/// reused.
fn changes(
    tracks: &[tessera_model::Track],
    vias: &[tessera_model::Via],
    board: &tessera_model::Board,
) -> Changes {
    let missing_tracks = |from: &[tessera_model::Track], to: &[tessera_model::Track]| {
        from.iter()
            .filter(|t| !to.iter().any(|o| o.id == t.id))
            .cloned()
            .collect()
    };
    let missing_vias = |from: &[tessera_model::Via], to: &[tessera_model::Via]| {
        from.iter()
            .filter(|v| !to.iter().any(|o| o.id == v.id))
            .cloned()
            .collect()
    };
    Changes {
        new_tracks: missing_tracks(&board.tracks, tracks),
        new_vias: missing_vias(&board.vias, vias),
        removed_tracks: missing_tracks(tracks, &board.tracks),
        removed_vias: missing_vias(vias, &board.vias),
    }
}

/// A `--plane LAYER=NET` argument, as names.
type PlaneArg = (String, String);

struct RouteArgs<'a> {
    input: &'a String,
    output: &'a String,
    planes: Vec<PlaneArg>,
    no_pours: bool,
    soft_fills: Vec<&'a String>,
    track_nets: Vec<&'a String>,
    new_items: Option<&'a String>,
    reroutable_from: Option<&'a String>,
}

/// `[--plane LAYER=NET]... [--new-items FILE] <input> <output>`.
fn parse_route_args(args: &[String]) -> Result<RouteArgs<'_>> {
    let mut plane_args = Vec::new();
    let mut new_items = None;
    let mut no_pours = false;
    let mut soft_fills = Vec::new();
    let mut track_nets = Vec::new();
    let mut reroutable_from = None;
    let mut positional = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--new-items" {
            new_items = Some(rest.next().context("--new-items needs a file path")?);
        } else if arg == "--soft-fill" {
            soft_fills.push(rest.next().context("--soft-fill needs a net name")?);
        } else if arg == "--reroutable-from" {
            reroutable_from = Some(
                rest.next()
                    .context("--reroutable-from needs the original .kicad_pcb")?,
            );
        } else if arg == "--no-pours" {
            no_pours = true;
        } else if arg == "--track-net" {
            track_nets.push(rest.next().context("--track-net needs a net name")?);
        } else if arg == "--plane" {
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
    Ok(RouteArgs {
        input,
        output,
        planes: plane_args,
        no_pours,
        soft_fills,
        track_nets,
        new_items,
        reroutable_from,
    })
}

/// Marks every track and via of `board` that isn't in the `base` board
/// file (the user's original, before any routing pass) as reroutable: it's
/// the router's own earlier output, which rip-up may take out again.
fn mark_reroutable(
    board: &mut tessera_model::Board,
    base: &str,
    pro_text: Option<&str>,
) -> Result<()> {
    let text = fs::read_to_string(base).with_context(|| format!("reading {base}"))?;
    let base = tessera_io_kicad::parser::parse_board(&text, pro_text)
        .with_context(|| format!("parsing {base}"))?
        .board;
    let layer =
        |b: &tessera_model::Board, id| b.layers.iter().find(|l| l.id == id).map(|l| l.name.clone());
    let same_track = |t: &tessera_model::Track, o: &tessera_model::Track| {
        let ends = (t.segment.a == o.segment.a && t.segment.b == o.segment.b)
            || (t.segment.a == o.segment.b && t.segment.b == o.segment.a);
        ends && t.width_nm == o.width_nm && layer(board, t.layer) == layer(&base, o.layer)
    };
    board.reroutable_tracks = board
        .tracks
        .iter()
        .filter(|t| !base.tracks.iter().any(|o| same_track(t, o)))
        .map(|t| t.id)
        .collect();
    board.reroutable_vias = board
        .vias
        .iter()
        .filter(|v| {
            !base
                .vias
                .iter()
                .any(|o| o.position == v.position && o.diameter_nm == v.diameter_nm)
        })
        .map(|v| v.id)
        .collect();
    Ok(())
}

/// Every net with a copper zone that isn't filled yet becomes a pour net,
/// except planes (already handled) and the `--track-net` ones. A filled
/// pour is real copper instead: its islands are checked for what they
/// join, and only the gaps are routed (see `find_unrouted_connections`).
fn set_pour_nets(board: &mut tessera_model::Board, track_nets: &[&String]) -> Result<()> {
    for name in track_nets {
        if !board.nets.values().any(|n| &&n.name == name) {
            bail!("--track-net: no net named {name}");
        }
    }
    board.pour_nets = board
        .zone_nets()
        .into_iter()
        .filter(|n| !board.is_plane_net(*n))
        .filter(|n| !board.zone_fills.iter().any(|f| f.net == *n))
        .filter(|n| !track_nets.iter().any(|t| board.nets[n].name == **t))
        .collect();
    Ok(())
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

/// The newly added items as JSON (board coordinates in nanometres, layers
/// and nets by name) — what the KiCad plugin creates in the open board.
/// The `--new-items` file: what this pass added — and, under
/// `--reroutable-from`, what of earlier passes' copper it ripped up.
fn new_items_json(
    board: &tessera_model::Board,
    (tracks, vias): (&[tessera_model::Track], &[tessera_model::Via]),
    (removed_tracks, removed_vias): (&[tessera_model::Track], &[tessera_model::Via]),
    planes: &[Plane],
) -> Result<String> {
    let layer_name = |id| {
        board
            .layers
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.name.clone())
            .context("an item on an unknown layer")
    };
    let net_name = |id| {
        board
            .nets
            .get(&id)
            .map(|n| n.name.clone())
            .context("an item on an unknown net")
    };
    let track_list = |tracks: &[tessera_model::Track]| {
        tracks
            .iter()
            .map(|t| {
                Ok(serde_json::json!({
                    "start": [t.segment.a.x, t.segment.a.y],
                    "end": [t.segment.b.x, t.segment.b.y],
                    "width": t.width_nm,
                    "layer": layer_name(t.layer)?,
                    "net": net_name(t.net)?,
                }))
            })
            .collect::<Result<Vec<_>>>()
    };
    let via_list = |vias: &[tessera_model::Via]| {
        vias.iter()
            .map(|v| {
                Ok(serde_json::json!({
                    "position": [v.position.x, v.position.y],
                    "diameter": v.diameter_nm,
                    "drill": v.drill_nm,
                    "net": net_name(v.net)?,
                }))
            })
            .collect::<Result<Vec<_>>>()
    };
    let outline: Vec<[i64; 2]> = board
        .outline
        .as_ref()
        .map(|o| o.vertices.iter().map(|p| [p.x, p.y]).collect())
        .unwrap_or_default();
    let zones = planes
        .iter()
        .map(|p| {
            let clearance = board
                .net_class_for(p.net)
                .map(|c| c.clearance_nm)
                .context("a plane on a net without a class")?;
            Ok(serde_json::json!({
                "net": net_name(p.net)?,
                "layer": layer_name(p.layer)?,
                "outline": outline,
                "clearance": clearance,
                "min_thickness": tessera_io_kicad::commit::PLANE_MIN_THICKNESS_NM,
                "thermal_gap": tessera_io_kicad::commit::PLANE_THERMAL_GAP_NM,
                "thermal_bridge_width": tessera_io_kicad::commit::PLANE_THERMAL_BRIDGE_NM,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "tracks": track_list(tracks)?,
        "vias": via_list(vias)?,
        "removed_tracks": track_list(removed_tracks)?,
        "removed_vias": via_list(removed_vias)?,
        "zones": zones,
    }))?)
}
