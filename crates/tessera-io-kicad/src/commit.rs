//! Surgical "commit new geometry" writer: splices newly-routed tracks/
//! vias into the *original* `.kicad_pcb` file's own text, leaving
//! everything else byte-for-byte untouched — copper pours, board
//! outline, silkscreen, stackup, UUIDs, every footprint, everything this
//! crate's parser doesn't even read. This is the actual "preserve
//! everything, add only what changed" production commit-back
//! `AUTOROUTER_PLAN.md` §2.3 describes, unlike [`crate::fixture`]'s
//! from-scratch minimal rewrite (built only for the DRC-parity test
//! harness — 2-layer-only, drops everything but pads/tracks/vias/net
//! classes).
//!
//! Deliberately narrow: only ever *adds* `(segment ...)`/`(via ...)`
//! items, inserted just before the root list's closing paren. Never
//! modifies, removes, or reorders anything already in the file — the
//! autorouter only ever routes previously-unrouted nets, never touches
//! existing tracks/vias/pads/zones/footprints, so there's nothing else
//! for this writer to do.
//!
//! Net references are written as name-only (`(net "GND")`), matching
//! the real board (`example/esp32-plc.kicad_pcb`, kept local-only — see
//! `.gitignore`) this was verified against — that file uses the same
//! name-only shape for every existing track/via's own `(net ...)` too,
//! not just pads (see `docs/DECISIONS.md`'s "Multi-layer parsing"
//! entry's sibling for the splice writer). **Not yet verified against an
//! older file using a numeric `(net N "name")` ID table** — plausible
//! that KiCad accepts name-only net references in either file version
//! (the underlying grammar doesn't obviously depend on it), but that's
//! an assumption, not a checked fact, until a board using the older
//! convention is available to test against.

use std::fmt::Write as _;

use tessera_model::{Board, LayerId, Plane, Track, Via};

const NM_PER_MM: f64 = 1_000_000.0;

// See fixture.rs's identical `mm` helper's doc comment: bounded by
// MAX_COORDINATE_NM in practice, so this f64 round-trip is lossless.
#[allow(clippy::cast_precision_loss)]
fn mm(nm: i64) -> String {
    format!("{:.6}", nm as f64 / NM_PER_MM)
}

/// A deterministic placeholder UUID for a newly-added item — sufficient
/// for KiCad's parser (doesn't verify UUID provenance) but not a real
/// UUID, matching `fixture.rs`'s own identical convention. `salt` lets a
/// caller keep new tracks' and new vias' UUIDs from colliding with each
/// other without needing a single shared counter threaded through both.
fn new_item_uuid(salt: &str, index: usize) -> String {
    format!("00000000-0000-0000-{salt}-{index:012x}")
}

fn layer_name(board: &Board, id: LayerId) -> Option<&str> {
    board
        .layers
        .iter()
        .find(|l| l.id == id)
        .map(|l| l.name.as_str())
}

/// A board-sized, *unfilled* copper zone for `plane`, its outline the board
/// outline, in the same shape KiCad 10 writes its own zones. KiCad fills it
/// on "Fill All Zones" (B), or `kicad-cli pcb drc --refill-zones`. Its
/// clearance is the plane net's own net-class clearance. The thermal gap is
/// 0.25mm, not KiCad's 0.5mm default: on a real board the 0.5mm gap left the
/// plane copper between a 2.54mm-pitch header's 1.7mm pins thinner than the
/// zone's minimum width, starving a pin's thermal spokes (`kicad-cli`
/// `starved_thermal`); 0.25mm matches the user's own zones and fixes it.
/// `None` without a known board outline.
/// Thermal-relief gap of a plane zone tessera adds — 0.25mm, not KiCad's
/// 0.5mm default (see [`plane_zone_text`]'s docs for why).
pub const PLANE_THERMAL_GAP_NM: i64 = 250_000;
/// Thermal spoke width of a plane zone tessera adds.
pub const PLANE_THERMAL_BRIDGE_NM: i64 = 500_000;
/// Minimum copper width of a plane zone tessera adds.
pub const PLANE_MIN_THICKNESS_NM: i64 = 250_000;

fn plane_zone_text(board: &Board, plane: Plane, index: usize) -> Option<String> {
    let outline = board.outline.as_ref()?;
    let layer = layer_name(board, plane.layer)?;
    let net = net_name(board, plane.net)?;
    let clearance_nm = board.net_class_for(plane.net)?.clearance_nm;
    let mut pts = String::new();
    for p in &outline.vertices {
        write!(pts, " (xy {} {})", mm(p.x), mm(p.y)).ok()?;
    }
    Some(format!(
        "\t(zone\n\t\t(net \"{net}\")\n\t\t(layer \"{layer}\")\n\t\t(uuid \"{}\")\n\t\t(hatch edge 0.5)\n\t\t(connect_pads\n\t\t\t(clearance {})\n\t\t)\n\t\t(min_thickness {})\n\t\t(fill yes\n\t\t\t(thermal_gap {})\n\t\t\t(thermal_bridge_width {})\n\t\t)\n\t\t(polygon\n\t\t\t(pts{pts})\n\t\t)\n\t)\n",
        new_item_uuid("20ae", index),
        mm(clearance_nm),
        mm(PLANE_MIN_THICKNESS_NM),
        mm(PLANE_THERMAL_GAP_NM),
        mm(PLANE_THERMAL_BRIDGE_NM),
    ))
}

fn net_name(board: &Board, id: tessera_model::NetId) -> Option<&str> {
    board.nets.get(&id).map(|n| n.name.as_str())
}

/// Splices `new_tracks`/`new_vias` into `original_pcb_text` as new
/// top-level `(segment ...)`/`(via ...)` items, inserted just before the
/// root list's closing paren — every other byte of the original text is
/// preserved exactly. `board` is only consulted for layer/net *names*
/// (`new_tracks`/`new_vias` themselves only carry `LayerId`/`NetId`),
/// not re-serialized itself.
///
/// Returns `None` if `original_pcb_text` doesn't end with `)` (not a
/// well-formed single-root `.kicad_pcb` file — the same shape
/// `crate::sexpr::parse` itself requires), or if any track/via
/// references a layer or net that isn't actually present on `board` (a
/// caller-contract violation: every item passed here should have come
/// from routing against this exact `board`, so this should never
/// trigger in practice — failing closed with `None` rather than writing
/// a `(layer "")`/malformed reference is the safe response if it
/// somehow does).
#[must_use]
pub fn splice_new_geometry(
    original_pcb_text: &str,
    board: &Board,
    new_tracks: &[Track],
    new_vias: &[Via],
    new_planes: &[Plane],
) -> Option<String> {
    let trimmed_end = original_pcb_text.trim_end();
    if !trimmed_end.ends_with(')') {
        return None;
    }
    let insert_at = trimmed_end.len() - 1;

    let mut new_items = String::new();
    for (i, track) in new_tracks.iter().enumerate() {
        let layer = layer_name(board, track.layer)?;
        let net = net_name(board, track.net)?;
        let locked = if track.locked {
            "\t\t(locked yes)\n"
        } else {
            ""
        };
        write!(
            new_items,
            "\t(segment\n\t\t(start {} {})\n\t\t(end {} {})\n\t\t(width {})\n\t\t(layer \"{layer}\")\n\t\t(net \"{net}\")\n{locked}\t\t(uuid \"{}\")\n\t)\n",
            mm(track.segment.a.x),
            mm(track.segment.a.y),
            mm(track.segment.b.x),
            mm(track.segment.b.y),
            mm(track.width_nm),
            new_item_uuid("5eg7", i),
        )
        .ok()?;
    }
    for (i, via) in new_vias.iter().enumerate() {
        let from = layer_name(board, via.from_layer)?;
        let to = layer_name(board, via.to_layer)?;
        let net = net_name(board, via.net)?;
        let locked = if via.locked { "\t\t(locked yes)\n" } else { "" };
        write!(
            new_items,
            "\t(via\n\t\t(at {} {})\n\t\t(size {})\n\t\t(drill {})\n\t\t(layers \"{from}\" \"{to}\")\n\t\t(net \"{net}\")\n{locked}\t\t(uuid \"{}\")\n\t)\n",
            mm(via.position.x),
            mm(via.position.y),
            mm(via.diameter_nm),
            mm(via.drill_nm),
            new_item_uuid("713a", i),
        )
        .ok()?;
    }

    for (i, plane) in new_planes.iter().enumerate() {
        new_items.push_str(&plane_zone_text(board, *plane, i)?);
    }

    let mut result = String::with_capacity(original_pcb_text.len() + new_items.len());
    result.push_str(&original_pcb_text[..insert_at]);
    result.push_str(&new_items);
    result.push_str(&original_pcb_text[insert_at..]);
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::{Point, Segment};
    use tessera_model::{Layer, LayerId, Net, NetClass, NetId, TrackId, ViaId};

    fn board_with_two_layers_and_one_net() -> Board {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
        board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
        board
            .net_classes
            .insert("Default".to_string(), NetClass::default_placeholder());
        board.nets.insert(
            NetId(1),
            Net {
                id: NetId(1),
                name: "GND".to_string(),
                net_class: "Default".to_string(),
            },
        );
        board
    }

    #[test]
    fn splices_a_new_track_before_the_closing_paren_leaving_everything_else_untouched() {
        let board = board_with_two_layers_and_one_net();
        let original = "(kicad_pcb\n\t(version 1)\n\t(some_untouched_thing 42)\n)\n";
        let track = Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(0, 0), Point::new(1_000_000, 0)),
            width_nm: 250_000,
            layer: LayerId(0),
            net: NetId(1),
            locked: false,
        };

        let spliced = splice_new_geometry(original, &board, &[track], &[], &[]).unwrap();

        assert!(
            spliced.contains("(some_untouched_thing 42)"),
            "original content must survive untouched"
        );
        assert!(spliced.contains("(segment"));
        assert!(spliced.contains("(layer \"F.Cu\")"));
        assert!(spliced.contains("(net \"GND\")"));
        // The new segment must land *inside* the root list, before its
        // closing paren, not appended after the file ends.
        let close_pos = spliced.rfind(')').unwrap();
        let segment_pos = spliced.find("(segment").unwrap();
        assert!(segment_pos < close_pos);
    }

    #[test]
    fn splices_a_new_via_with_the_right_layer_span() {
        let board = board_with_two_layers_and_one_net();
        let original = "(kicad_pcb\n\t(version 1)\n)\n";
        let via = Via {
            id: ViaId(0),
            position: Point::new(500_000, 500_000),
            diameter_nm: 600_000,
            drill_nm: 300_000,
            from_layer: LayerId(0),
            to_layer: LayerId(1),
            net: NetId(1),
            locked: false,
        };

        let spliced = splice_new_geometry(original, &board, &[], &[via], &[]).unwrap();
        assert!(spliced.contains("(via"));
        assert!(spliced.contains("(layers \"F.Cu\" \"B.Cu\")"));
        assert!(spliced.contains("(net \"GND\")"));
    }

    #[test]
    fn preserves_the_original_bytes_exactly_outside_the_insertion_point() {
        let board = board_with_two_layers_and_one_net();
        let original =
            "(kicad_pcb\n\t(version 1)\n\t(zone (weird \"formatting\"\n\t\t(nested 1)\n\t))\n)\n";
        let spliced = splice_new_geometry(original, &board, &[], &[], &[]).unwrap();
        // With no new items at all, splicing should be a no-op.
        assert_eq!(spliced, original);
    }

    #[test]
    fn returns_none_for_text_that_does_not_end_with_a_closing_paren() {
        let board = board_with_two_layers_and_one_net();
        // Genuinely truncated (no closing paren anywhere at the end) —
        // not just unbalanced, since this function's guard only checks
        // the last character, matching what it's actually meant to catch
        // (a caller that didn't validate the text first).
        assert!(splice_new_geometry("(kicad_pcb (version 1", &board, &[], &[], &[]).is_none());
    }

    #[test]
    fn returns_none_when_a_track_references_a_layer_not_on_the_board() {
        let board = board_with_two_layers_and_one_net();
        let original = "(kicad_pcb\n\t(version 1)\n)\n";
        let track = Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(0, 0), Point::new(1_000_000, 0)),
            width_nm: 250_000,
            layer: LayerId(99),
            net: NetId(1),
            locked: false,
        };
        assert!(splice_new_geometry(original, &board, &[track], &[], &[]).is_none());
    }
}
