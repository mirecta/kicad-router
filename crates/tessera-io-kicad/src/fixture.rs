//! `.kicad_pcb` / `.kicad_pro` fixture generation for the M1 DRC parity
//! harness (plan §3.2).
//!
//! **This is not the production commit-back writer.** The real "commit"
//! step (plan §2.3 step 3) has requirements this module doesn't attempt:
//! preserving existing item UUIDs, round-tripping everything the parser
//! read rather than a from-scratch minimal file, honouring KiCad's full
//! layer-ordinal convention for arbitrary stackups, etc. This module exists
//! to answer one narrower question — "does `tessera-drc`'s clearance
//! verdict match KiCad's own DRC on this geometry?" — by emitting the
//! smallest file `kicad-cli pcb drc` will accept.
//!
//! File-format details below were verified empirically against a real
//! KiCad 10.0.3 install (`kicad-cli pcb drc --format json`), not assumed:
//! net-class clearance lives in the companion `.kicad_pro` JSON
//! (`net_settings.classes`), not in `.kicad_pcb`'s `setup` section; pads
//! only exist nested inside `footprint` elements; pad/track/via geometry in
//! `.kicad_pcb` is in millimetres (floating point), unlike this workspace's
//! internal integer-nanometre representation.
//!
//! Scoped to exactly two copper layers (`F.Cu` ordinal 0, `B.Cu` ordinal 2 —
//! KiCad's fixed numbering for the outer layers) for now: `tessera_model`
//! doesn't yet encode KiCad's inner-layer ordinal convention
//! (`4, 6, 8, ...` in stackup order), so boards with more than two
//! [`tessera_model::Layer`]s aren't supported here yet.

use std::fmt::Write as _;

use tessera_geom::Point;
use tessera_model::{Board, LayerId, NetClass, PadShape};

const NM_PER_MM: f64 = 1_000_000.0;

// Coordinates are bounded by MAX_COORDINATE_NM (1e9), comfortably inside
// f64's exact-integer range (2^52 ~ 4.5e15) — this conversion to millimetre
// text is lossless in practice, not the general lossy i64->f64 cast clippy
// warns about.
#[allow(clippy::cast_precision_loss)]
fn mm(nm: i64) -> String {
    format!("{:.6}", nm as f64 / NM_PER_MM)
}

/// A deterministic placeholder UUID, sufficient for KiCad's parser (which
/// doesn't verify UUID provenance) but **not** a real UUID — do not reuse
/// this for anything that needs actual identity stability.
fn fixture_uuid(n: u64) -> String {
    format!("00000000-0000-0000-0000-{n:012x}")
}

fn locked_clause(locked: bool) -> &'static str {
    if locked {
        "\t\t(locked yes)\n"
    } else {
        ""
    }
}

fn kicad_layer_name(id: LayerId) -> &'static str {
    // See module docs: only the two-layer case is supported so far.
    if id.0 == 0 {
        "F.Cu"
    } else {
        "B.Cu"
    }
}

/// Renders `board` as a `(pcb_text, pro_text)` pair of file contents.
/// Write them to `<name>.kicad_pcb` and `<name>.kicad_pro` in the same
/// directory (KiCad associates a board with its project by matching
/// filename stem) before running `kicad-cli pcb drc` on the `.kicad_pcb`.
///
/// # Panics
///
/// Panics if `board` has more than two layers, or any layer other than the
/// two expected copper layers — see the module-level scope note.
#[must_use]
pub fn write_fixture(board: &Board) -> (String, String) {
    assert!(
        board.layers.len() <= 2,
        "fixture writer only supports 2-layer boards for now"
    );

    let mut uuid_counter = 0u64;
    let mut next_uuid = || {
        uuid_counter += 1;
        fixture_uuid(uuid_counter)
    };

    let mut pcb = String::new();
    pcb.push_str("(kicad_pcb\n");
    pcb.push_str("\t(version 20241229)\n");
    pcb.push_str("\t(generator \"tessera\")\n");
    pcb.push_str("\t(generator_version \"0.1\")\n");
    pcb.push_str("\t(general\n\t\t(thickness 1.6)\n\t)\n");
    pcb.push_str("\t(paper \"A4\")\n");
    pcb.push_str(
        "\t(layers\n\t\t(0 \"F.Cu\" signal)\n\t\t(2 \"B.Cu\" signal)\n\t\t(25 \"Edge.Cuts\" user)\n\t)\n",
    );
    pcb.push_str("\t(setup\n\t\t(pad_to_mask_clearance 0)\n\t)\n");
    pcb.push_str("\t(net 0 \"\")\n");
    for net in board.nets.values() {
        let _ = writeln!(pcb, "\t(net {} \"{}\")", net.id.0, net.name);
    }

    for track in &board.tracks {
        let _ = write!(
            pcb,
            "\t(segment\n\t\t(start {} {})\n\t\t(end {} {})\n\t\t(width {})\n\t\t(layer \"{}\")\n\t\t(net {})\n{}\t\t(uuid \"{}\")\n\t)\n",
            mm(track.segment.a.x),
            mm(track.segment.a.y),
            mm(track.segment.b.x),
            mm(track.segment.b.y),
            mm(track.width_nm),
            kicad_layer_name(track.layer),
            track.net.0,
            locked_clause(track.locked),
            next_uuid(),
        );
    }

    for via in &board.vias {
        let _ = write!(
            pcb,
            "\t(via\n\t\t(at {} {})\n\t\t(size {})\n\t\t(drill {})\n\t\t(layers \"F.Cu\" \"B.Cu\")\n\t\t(net {})\n{}\t\t(uuid \"{}\")\n\t)\n",
            mm(via.position.x),
            mm(via.position.y),
            mm(via.diameter_nm),
            mm(via.drill_nm),
            via.net.0,
            locked_clause(via.locked),
            next_uuid(),
        );
    }

    for pad in &board.pads {
        let (center, kicad_shape, width_nm, height_nm, extra) = kicad_pad_geometry(&pad.shape);
        let net_name = board.nets.get(&pad.net).map_or("", |n| n.name.as_str());
        // A pad on both copper layers with a known circular drill goes back
        // out as a real `thru_hole` pad. Without a known drill (e.g. a
        // slotted hole, which `Pad::drill_nm` can't represent) its copper
        // is written per layer instead, rather than inventing a drill size.
        let through_hole_drill = pad.drill_nm.filter(|_| pad.layers.len() > 1);
        let pads_to_write: Vec<(&str, &str, String, String)> = match through_hole_drill {
            Some(drill_nm) => vec![(
                "F.Cu",
                "through_hole",
                format!("thru_hole {kicad_shape}\n\t\t\t(drill {})", mm(drill_nm)),
                "\"*.Cu\" \"*.Mask\"".to_string(),
            )],
            None => pad
                .layers
                .iter()
                .map(|&l| {
                    let name = kicad_layer_name(l);
                    (
                        name,
                        "smd",
                        format!("smd {kicad_shape}"),
                        format!("\"{name}\""),
                    )
                })
                .collect(),
        };
        for (fp_layer, attr, pad_kind, pad_layers) in pads_to_write {
            let _ = write!(
                pcb,
                "\t(footprint \"tessera:single_pad\"\n\t\t(layer \"{fp_layer}\")\n\t\t(uuid \"{}\")\n\t\t(at {} {})\n{}\t\t(attr {attr})\n\t\t(pad \"1\" {pad_kind}\n\t\t\t(at 0 0)\n\t\t\t(size {} {}){extra}\n\t\t\t(layers {pad_layers})\n\t\t\t(net {} \"{net_name}\")\n\t\t\t(uuid \"{}\")\n\t\t)\n\t)\n",
                next_uuid(),
                mm(center.x),
                mm(center.y),
                locked_clause(pad.locked),
                mm(width_nm),
                mm(height_nm),
                pad.net.0,
                next_uuid(),
            );
        }
    }

    write_holes(&mut pcb, board, &mut next_uuid);
    write_zone_fills(&mut pcb, board, &mut next_uuid);
    write_outline_and_keepouts(&mut pcb, board, &mut next_uuid);

    pcb.push(')');

    let pro = write_project(board);
    (pcb, pro)
}

/// A pad shape as KiCad's `(at x y)` centre, shape keyword, and `(size W H)`.
/// A pad shape as KiCad's `(at x y)` centre, shape keyword, `(size W H)`,
/// and any extra body text (a custom pad's polygon primitive).
fn kicad_pad_geometry(shape: &PadShape) -> (Point, &'static str, i64, i64, String) {
    match shape {
        PadShape::Circle(c) => (
            c.center,
            "circle",
            c.radius_nm * 2,
            c.radius_nm * 2,
            String::new(),
        ),
        PadShape::Rect(r) => (
            r.center,
            "rect",
            r.half_width_nm * 2,
            r.half_height_nm * 2,
            String::new(),
        ),
        PadShape::Polygon { polygon, center } => {
            let mut pts = String::new();
            for p in &polygon.vertices {
                let _ = write!(pts, " (xy {} {})", mm(p.x - center.x), mm(p.y - center.y));
            }
            let extra = format!(
                "\n\t\t\t(options (clearance outline) (anchor circle))\n\t\t\t(primitives (gr_poly (pts{pts}) (width 0) (fill yes)))"
            );
            (*center, "custom", 100_000, 100_000, extra)
        }
        PadShape::Oval(o) => {
            // The spine is always axis-aligned (see `Oval::new`), so
            // its length is an exact absolute difference — no need
            // for `length_nm`'s reporting-only sqrt.
            let along_x = (o.spine.b.x - o.spine.a.x).abs();
            let along_y = (o.spine.b.y - o.spine.a.y).abs();
            let (width_nm, height_nm) = if along_x >= along_y {
                (along_x + o.radius_nm * 2, o.radius_nm * 2)
            } else {
                (o.radius_nm * 2, along_y + o.radius_nm * 2)
            };
            (o.center, "oval", width_nm, height_nm, String::new())
        }
    }
}

/// Non-plated holes, as single-pad `np_thru_hole` footprints.
fn write_holes(pcb: &mut String, board: &Board, next_uuid: &mut impl FnMut() -> String) {
    for hole in &board.npth_holes {
        let d = mm(hole.diameter_nm);
        let _ = write!(
            pcb,
            "\t(footprint \"tessera:npth\"\n\t\t(layer \"F.Cu\")\n\t\t(uuid \"{}\")\n\t\t(at {} {})\n\t\t(attr exclude_from_pos_files exclude_from_bom)\n\t\t(pad \"\" np_thru_hole circle\n\t\t\t(at 0 0)\n\t\t\t(size {d} {d})\n\t\t\t(drill {d})\n\t\t\t(layers \"*.Cu\" \"*.Mask\")\n\t\t\t(uuid \"{}\")\n\t\t)\n\t)\n",
            next_uuid(),
            mm(hole.position.x),
            mm(hole.position.y),
            next_uuid(),
        );
    }
}

/// Each zone fill as its own zone whose outline is the fill itself. This
/// writer emits a pre-`20250210` file version, where KiCad grows a stored
/// fill by half the zone's `min_thickness` — so `min_thickness` is written
/// as twice `inflation_nm`, and a `0` gives an exact fill.
fn write_zone_fills(pcb: &mut String, board: &Board, next_uuid: &mut impl FnMut() -> String) {
    for fill in &board.zone_fills {
        let layer = kicad_layer_name(fill.layer);
        let mut pts = String::new();
        for p in &fill.polygon.vertices {
            let _ = write!(pts, " (xy {} {})", mm(p.x), mm(p.y));
        }
        let net_name = board.nets.get(&fill.net).map_or("", |n| n.name.as_str());
        let _ = write!(
            pcb,
            "\t(zone (net {}) (net_name \"{net_name}\") (layer \"{layer}\") (uuid \"{}\") (hatch edge 0.5)\n\t\t(connect_pads (clearance {}))\n\t\t(min_thickness {})\n\t\t(fill yes (thermal_gap 0.5) (thermal_bridge_width 0.5))\n\t\t(polygon (pts{pts}))\n\t\t(filled_polygon (layer \"{layer}\") (pts{pts}))\n\t)\n",
            fill.net.0,
            next_uuid(),
            mm(fill.clearance_nm),
            mm(2 * fill.inflation_nm),
        );
    }
}

/// The board outline as `Edge.Cuts` lines, and each keepout rule area as
/// a named keepout zone.
fn write_outline_and_keepouts(
    pcb: &mut String,
    board: &Board,
    next_uuid: &mut impl FnMut() -> String,
) {
    if let Some(outline) = &board.outline {
        let n = outline.vertices.len();
        for i in 0..n {
            let (a, b) = (outline.vertices[i], outline.vertices[(i + 1) % n]);
            let _ = writeln!(
                pcb,
                "\t(gr_line (start {} {}) (end {} {}) (stroke (width 0.05) (type solid)) (layer \"Edge.Cuts\") (uuid \"{}\"))",
                mm(a.x),
                mm(a.y),
                mm(b.x),
                mm(b.y),
                next_uuid(),
            );
        }
    }
    let allowed = |yes: bool| if yes { "allowed" } else { "not_allowed" };
    for area in &board.rule_areas {
        let mut layers = String::new();
        for &l in &area.layers {
            let _ = write!(layers, " \"{}\"", kicad_layer_name(l));
        }
        let mut pts = String::new();
        for p in &area.outline.vertices {
            let _ = write!(pts, " (xy {} {})", mm(p.x), mm(p.y));
        }
        let k = &area.keepout;
        let _ = write!(
            pcb,
            "\t(zone (net 0) (net_name \"\") (layers{layers}) (uuid \"{}\") (name \"{}\") (hatch edge 0.5)\n\t\t(connect_pads (clearance 0)) (min_thickness 0.25)\n\t\t(keepout (tracks {}) (vias {}) (pads {}) (copperpour {}) (footprints {}))\n\t\t(fill (thermal_gap 0.5) (thermal_bridge_width 0.5))\n\t\t(polygon (pts{pts}))\n\t)\n",
            next_uuid(),
            area.name,
            allowed(k.tracks_allowed),
            allowed(k.vias_allowed),
            allowed(k.pads_allowed),
            allowed(k.copper_pour_allowed),
            allowed(k.footprints_allowed),
        );
    }
}

fn write_project(board: &Board) -> String {
    let mut classes = String::new();
    for (i, class) in board.net_classes.values().enumerate() {
        if i > 0 {
            classes.push_str(",\n");
        }
        classes.push_str(&class_json(class));
    }

    // Board-level minimum-constraint rules (`min_track_width`, `min_clearance`,
    // etc.) default to KiCad's own hardcoded values (e.g. 0.2mm min track
    // width) when omitted here — verified empirically against a real
    // project file (`EuroCard160mmX100mm.kicad_pro`). The ones `Board::rules`
    // models are written from it; every other one is pinned to zero so
    // board-level rules `tessera-drc` doesn't know about can't silently
    // interfere with the harness.
    let rules = board.rules;
    let hole_clearance = mm(rules.min_hole_clearance_nm);
    let hole_to_hole = mm(rules.min_hole_to_hole_nm);
    let annular = mm(rules.min_via_annular_width_nm);
    let edge = mm(rules.min_copper_edge_clearance_nm);
    // Every net not in "Default" is assigned its class explicitly — the
    // project file, not the board, is where KiCad 8+ keeps that. Sorted for
    // a deterministic file.
    let mut assigned: Vec<(&str, &str)> = board
        .nets
        .values()
        .filter(|n| n.net_class != "Default")
        .map(|n| (n.name.as_str(), n.net_class.as_str()))
        .collect();
    assigned.sort_unstable();
    let assignments = if assigned.is_empty() {
        "null".to_string()
    } else {
        let entries: Vec<String> = assigned
            .iter()
            .map(|(net, class)| format!("\"{net}\": \"{class}\""))
            .collect();
        format!("{{ {} }}", entries.join(", "))
    };
    format!(
        "{{\n  \"net_settings\": {{\n    \"classes\": [\n{classes}\n    ],\n    \"meta\": {{ \"version\": 4 }},\n    \"net_colors\": null,\n    \"netclass_assignments\": {assignments},\n    \"netclass_patterns\": []\n  }},\n  \"board\": {{ \"design_settings\": {{ \"rules\": {{ \"min_clearance\": 0.0, \"min_track_width\": 0.0, \"min_via_diameter\": 0.0, \"min_through_hole_diameter\": 0.0, \"min_copper_edge_clearance\": {edge}, \"min_hole_clearance\": {hole_clearance}, \"min_hole_to_hole\": {hole_to_hole}, \"min_via_annular_width\": {annular} }} }} }}\n}}\n"
    )
}

// See `mm`'s doc comment: these values are bounded well within f64's
// exact-integer range, so this cast is lossless in practice.
#[allow(clippy::cast_precision_loss)]
fn class_json(class: &NetClass) -> String {
    format!(
        "      {{ \"name\": \"{}\", \"clearance\": {}, \"track_width\": {}, \"via_diameter\": {}, \"via_drill\": {} }}",
        class.name,
        class.clearance_nm as f64 / NM_PER_MM,
        class.track_width_nm as f64 / NM_PER_MM,
        class.via_diameter_nm as f64 / NM_PER_MM,
        class.via_drill_nm as f64 / NM_PER_MM,
    )
}
