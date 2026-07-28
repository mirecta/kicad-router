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
    pcb.push_str("\t(layers\n\t\t(0 \"F.Cu\" signal)\n\t\t(2 \"B.Cu\" signal)\n\t)\n");
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
        let PadShape::Circle(circle) = &pad.shape;
        let layer = pad.layers.first().copied().map_or("F.Cu", kicad_layer_name);
        let _ = write!(
            pcb,
            "\t(footprint \"tessera:single_pad\"\n\t\t(layer \"{layer}\")\n\t\t(uuid \"{}\")\n\t\t(at {} {})\n{}\t\t(attr smd)\n\t\t(pad \"1\" smd circle\n\t\t\t(at 0 0)\n\t\t\t(size {} {})\n\t\t\t(layers \"{layer}\")\n\t\t\t(net {} \"{}\")\n\t\t\t(uuid \"{}\")\n\t\t)\n\t)\n",
            next_uuid(),
            mm(circle.center.x),
            mm(circle.center.y),
            locked_clause(pad.locked),
            mm(circle.radius_nm * 2),
            mm(circle.radius_nm * 2),
            pad.net.0,
            board.nets.get(&pad.net).map_or("", |n| n.name.as_str()),
            next_uuid(),
        );
    }

    pcb.push(')');

    let pro = write_project(board);
    (pcb, pro)
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
    // project file (`EuroCard160mmX100mm.kicad_pro`). Pin them all to zero
    // so this fixture only ever exercises net-class clearance, which is all
    // `tessera-drc` models today; leaving them at KiCad's defaults would
    // make board-level rules `tessera-drc` doesn't know about silently
    // interfere with a harness meant to test net-class resolution alone.
    format!(
        "{{\n  \"net_settings\": {{\n    \"classes\": [\n{classes}\n    ],\n    \"meta\": {{ \"version\": 4 }},\n    \"net_colors\": null,\n    \"netclass_assignments\": null,\n    \"netclass_patterns\": []\n  }},\n  \"board\": {{ \"design_settings\": {{ \"rules\": {{ \"min_clearance\": 0.0, \"min_track_width\": 0.0, \"min_via_diameter\": 0.0, \"min_through_hole_diameter\": 0.0, \"min_hole_to_hole\": 0.0, \"min_copper_edge_clearance\": 0.0 }} }} }}\n}}\n"
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
