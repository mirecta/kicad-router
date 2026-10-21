//! Semantic extraction of a [`Board`] from `.kicad_pcb` + optional
//! companion `.kicad_pro` text.
//!
//! Scope: any number of copper layers (2026-08-21, see
//! [`parse_layers`]'s docs for the stackup-order subtlety this needed to
//! get right — previously hardcoded to exactly `F.Cu`/`B.Cu`, silently
//! dropping every inner layer's content), straight tracks only (no
//! arcs), through vias only (no blind/buried — a through via still must
//! span the *entire* stack, `F.Cu` to `B.Cu`, regardless of how many
//! inner layers that spans), circular/axis-aligned-rect pads only.
//! [`crate::fixture`]'s writer is still 2-layer-only (a distinct, not
//! yet closed, gap — see that module's own scope note). Footprint
//! rotation **is** applied
//! to pad position and (for `Rect`) shape orientation (2026-08-17, see
//! [`rotate_90_multiple`]'s docs) — but only for the four axis-preserving
//! multiples of 90°; any other angle (footprint's own, or a pad's local
//! angle composed with it) is an honest skip, not an approximated
//! rotation. Real boards using any of these are only partially ingested —
//! see [`ParsedBoard::warnings`] for what got dropped, rather than
//! silently losing fidelity with no trace. Net class assignment is
//! `"Default"` for every net; KiCad's `netclass_patterns`/
//! `netclass_assignments` matching isn't implemented.

use std::collections::HashMap;

use tessera_geom::{Circle, Oval, Point, Polygon, Rect, Segment};
use tessera_model::{
    Board, CopperZone, DesignRules, Hole, HoleId, KeepoutFlags, Layer, LayerId, Net, NetClass,
    NetId, Pad, PadId, PadShape, RuleArea, RuleAreaId, Track, TrackId, Via, ViaId, ZoneFill,
};

use crate::sexpr::{self, Sexpr};

#[derive(Debug, thiserror::Error)]
pub enum ParseBoardError {
    #[error("failed to parse .kicad_pcb syntax: {0}")]
    Syntax(#[from] sexpr::ParseError),
    #[error("failed to parse .kicad_pro as JSON: {0}")]
    ProjectJson(#[from] serde_json::Error),
    #[error("root element is not (kicad_pcb ...)")]
    NotAKicadPcb,
    #[error("board does not have both F.Cu and B.Cu copper layers")]
    MissingCopperLayers,
}

pub struct ParsedBoard {
    pub board: Board,
    /// Anything skipped rather than represented (unsupported pad shapes,
    /// malformed entries, footprints with unreadable positions) — surfaced
    /// explicitly rather than silently dropped, per the crate's no-silent-
    /// data-loss stance.
    pub warnings: Vec<String>,
}

/// Parses `pcb_text` (a `.kicad_pcb` file's contents) into a [`Board`],
/// optionally reading net-class definitions from `pro_text` (the companion
/// `.kicad_pro` project file's contents — KiCad stores net-class clearance/
/// width there, not in the board file itself; see `docs/DECISIONS.md`
/// ADR-0002). If `pro_text` is `None`, a single placeholder `"Default"`
/// class is used.
///
/// # Errors
///
/// Returns an error if `pcb_text` isn't valid S-expression syntax, its root
/// isn't `(kicad_pcb ...)`, it lacks both F.Cu and B.Cu copper layers, or
/// `pro_text` is present but isn't valid JSON.
pub fn parse_board(pcb_text: &str, pro_text: Option<&str>) -> Result<ParsedBoard, ParseBoardError> {
    let root = sexpr::parse(pcb_text)?;
    if root.head() != Some("kicad_pcb") {
        return Err(ParseBoardError::NotAKicadPcb);
    }

    let mut warnings = Vec::new();
    let mut board = Board::new();

    let layer_map = parse_layers(&root, &mut board)?;
    parse_net_classes(pro_text, &mut board)?;
    let name_to_id = parse_nets(&root, &mut board);
    parse_tracks(&root, &mut board, &name_to_id, &layer_map, &mut warnings);
    parse_vias(&root, &mut board, &name_to_id, &layer_map, &mut warnings);
    parse_pads(&root, &mut board, &name_to_id, &layer_map, &mut warnings);
    parse_rule_areas(&root, &mut board, &layer_map, &mut warnings);
    parse_zone_fills(&root, &mut board, &name_to_id, &layer_map, &mut warnings);
    board.outline = parse_board_outline(&root, &mut warnings);

    Ok(ParsedBoard { board, warnings })
}

fn resolve_layer_id(name: &str, layer_map: &HashMap<String, LayerId>) -> Option<LayerId> {
    layer_map.get(name).copied()
}

/// Resolves one `(layers ...)` token to every `LayerId` it refers to.
/// Usually a single exact name (`"F.Cu"`) resolving to at most one layer,
/// but KiCad's own file format also lets `"*.Cu"` stand for "every copper
/// layer on the board" — the standard shorthand for a through-hole pad
/// that spans the whole stack, rather than listing each copper layer by
/// name. Verified against the real board this crate is tested against
/// (`example/esp32-plc.kicad_pcb`, kept local-only — see `.gitignore`):
/// 86 of its through-hole pads (headers/connectors, all carrying real
/// nets, not decorative) use exactly this shorthand, and were silently
/// skipped entirely — `resolve_layer_id` alone can't match a literal
/// `"*.Cu"` against any real layer name, so the pad's whole `layers` list
/// came back empty and `parse_one_pad` bailed. `layer_map`'s values are
/// exactly the board's copper layers, with `LayerId`s assigned in
/// physical stackup order (see `parse_layers`'s doc comment), so sorting
/// them recovers that order — not that the order matters to any of this
/// module's callers, which only ever do membership checks.
fn resolve_layer_ids(name: &str, layer_map: &HashMap<String, LayerId>) -> Vec<LayerId> {
    if name == "*.Cu" {
        let mut ids: Vec<LayerId> = layer_map.values().copied().collect();
        ids.sort_unstable();
        ids
    } else {
        resolve_layer_id(name, layer_map).into_iter().collect()
    }
}

fn is_locked(item: &Sexpr) -> bool {
    item.find("locked").and_then(|l| l.atom(1)) == Some("yes")
}

// mm text (as found in .kicad_pcb / .kicad_pro) -> integer nanometres.
// Values are bounded by MAX_COORDINATE_NM in practice, so the round-trip
// through f64 is lossless; the truncation clippy warns about generically
// is already handled by the explicit `.round()`.
#[allow(clippy::cast_possible_truncation)]
fn nm(mm_str: &str) -> Option<i64> {
    let value: f64 = mm_str.parse().ok()?;
    Some((value * 1_000_000.0).round() as i64)
}

/// Reads every real copper layer from `(layers ...)` — `(<ordinal>
/// "<name>" <type> ...)` entries whose `<type>` is `signal` — into
/// `board.layers`, in **physical stackup order**, and returns the
/// `name -> LayerId` map every other layer-name lookup in this module
/// needs (`LayerId`s are freshly assigned here in that order, not copied
/// from KiCad's own ordinals).
///
/// KiCad's own ordinal numbering is *not* stackup order: `F.Cu` is
/// always ordinal `0`, `B.Cu` is always ordinal `2` regardless of how
/// many inner layers exist, and inner layers are numbered `4, 6, 8, ...`
/// in between — so `B.Cu`'s ordinal (`2`) sorts *before* every inner
/// layer's, even though it's physically last. Verified against a real
/// 4-layer board (`example/esp32-plc.kicad_pcb`, not committed to the
/// corpus — see `docs/DECISIONS.md`'s "Multi-layer parsing" entry):
/// `(0 "F.Cu" signal) (4 "In1.Cu" signal) (6 "In2.Cu" signal)
/// (2 "B.Cu" signal)`. This function sorts `F.Cu` first, `B.Cu` last,
/// and everything else by its own ordinal in between, rather than a
/// plain numeric sort.
///
/// Only `signal` is treated as copper — `power`/`mixed` (KiCad layer
/// types that exist for buried plane layers) haven't been observed in
/// any board this crate has actually parsed, so they're not guessed at
/// here; a board using one would report a plane layer as missing
/// entirely (an honest gap, not silently wrong).
fn parse_layers(
    root: &Sexpr,
    board: &mut Board,
) -> Result<HashMap<String, LayerId>, ParseBoardError> {
    let layers_list = root
        .find("layers")
        .ok_or(ParseBoardError::MissingCopperLayers)?;
    let entries = layers_list.as_list().unwrap_or(&[]);

    let mut copper: Vec<(i64, String)> = Vec::new();
    for entry in entries {
        if entry.atom(2) != Some("signal") {
            continue;
        }
        let Some(ordinal) = entry.atom(0).and_then(|s| s.parse::<i64>().ok()) else {
            continue;
        };
        let Some(name) = entry.atom(1) else { continue };
        copper.push((ordinal, name.to_string()));
    }

    let found_front = copper.iter().any(|(_, name)| name == "F.Cu");
    let found_back = copper.iter().any(|(_, name)| name == "B.Cu");
    if !found_front || !found_back {
        return Err(ParseBoardError::MissingCopperLayers);
    }

    copper.sort_by_key(|(ordinal, name)| match name.as_str() {
        "F.Cu" => i64::MIN,
        "B.Cu" => i64::MAX,
        _ => *ordinal,
    });

    let mut layer_map = HashMap::new();
    for (index, (_, name)) in copper.into_iter().enumerate() {
        let id = LayerId(u32::try_from(index).unwrap_or(u32::MAX));
        board.layers.push(Layer::copper(id, name.clone()));
        layer_map.insert(name, id);
    }
    Ok(layer_map)
}

#[allow(clippy::cast_possible_truncation)]
fn mm_from_json(obj: &serde_json::Value, key: &str) -> Option<i64> {
    let value = obj.get(key)?.as_f64()?;
    Some((value * 1_000_000.0).round() as i64)
}

fn parse_net_classes(pro_text: Option<&str>, board: &mut Board) -> Result<(), ParseBoardError> {
    if let Some(text) = pro_text {
        let json: serde_json::Value = serde_json::from_str(text)?;
        if let Some(classes) = json
            .pointer("/net_settings/classes")
            .and_then(serde_json::Value::as_array)
        {
            for class in classes {
                let name = class
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Default")
                    .to_string();
                board.net_classes.insert(
                    name.clone(),
                    NetClass {
                        name,
                        clearance_nm: mm_from_json(class, "clearance").unwrap_or(200_000),
                        track_width_nm: mm_from_json(class, "track_width").unwrap_or(250_000),
                        via_diameter_nm: mm_from_json(class, "via_diameter").unwrap_or(600_000),
                        via_drill_nm: mm_from_json(class, "via_drill").unwrap_or(300_000),
                        diff_pair_track_width_nm: mm_from_json(class, "diff_pair_width"),
                        diff_pair_gap_nm: mm_from_json(class, "diff_pair_gap"),
                        diff_pair_via_gap_nm: mm_from_json(class, "diff_pair_via_gap"),
                    },
                );
            }
        }
        if let Some(rules) = json.pointer("/board/design_settings/rules") {
            let defaults = board.rules;
            board.rules = DesignRules {
                min_hole_clearance_nm: mm_from_json(rules, "min_hole_clearance")
                    .unwrap_or(defaults.min_hole_clearance_nm),
                min_hole_to_hole_nm: mm_from_json(rules, "min_hole_to_hole")
                    .unwrap_or(defaults.min_hole_to_hole_nm),
                min_via_annular_width_nm: mm_from_json(rules, "min_via_annular_width")
                    .unwrap_or(defaults.min_via_annular_width_nm),
                min_copper_edge_clearance_nm: mm_from_json(rules, "min_copper_edge_clearance")
                    .unwrap_or(defaults.min_copper_edge_clearance_nm),
            };
        }
    }
    board
        .net_classes
        .entry("Default".to_string())
        .or_insert_with(NetClass::default_placeholder);
    Ok(())
}

/// Reads every net this board references and returns a name→[`NetId`]
/// lookup for [`resolve_net_id`] to use when parsing tracks/vias/pads.
///
/// Two `.kicad_pcb` net-reference shapes are handled, verified against a
/// real board (a user-supplied 4-layer, unrouted design saved by a
/// newer KiCad than this crate had seen before — see
/// `docs/DECISIONS.md`'s "Newer `.kicad_pcb` name-only net references"
/// entry):
/// - **The classic shape**: a real top-level `(net <id> "<name>")`
///   table, with every item elsewhere in the file referencing a net by
///   that same numeric `<id>` (`(net <id>)` on a pad/track/via). `NetId`
///   here is exactly KiCad's own ID.
/// - **A newer shape this crate hadn't encountered until now**: no
///   top-level table at all — every item references its net purely by
///   name (`(net "<name>")`, no ID). `NetId` is synthesized here
///   (assigned in first-seen document order, starting at 1) since the
///   file simply doesn't carry one.
///
/// A file only ever uses one shape throughout (never a mix in the same
/// file, as far as this crate has seen) — detected by whether the very
/// first `(net ...)` this function finds has a numeric second element.
fn parse_nets(root: &Sexpr, board: &mut Board) -> HashMap<String, NetId> {
    let mut name_to_id = HashMap::new();
    let mut found_id_table = false;
    for net_entry in root.find_all("net") {
        let Some(id) = net_entry.atom(1).and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        found_id_table = true;
        if id == 0 {
            continue; // net 0 is KiCad's "no net" sentinel.
        }
        let name = net_entry.atom(2).unwrap_or("").to_string();
        name_to_id.insert(name.clone(), NetId(id));
        board.nets.insert(
            NetId(id),
            Net {
                id: NetId(id),
                name,
                net_class: "Default".to_string(),
            },
        );
    }
    if found_id_table {
        return name_to_id;
    }

    // Newer, name-only shape: no top-level table to read IDs from at
    // all, so synthesize them — first-seen order across the whole
    // document (footprints/pads are nested, so this is a real recursive
    // walk, not `find_all`'s direct-children-only scan).
    let mut names = Vec::new();
    collect_name_only_net_refs(root, &mut names);
    let mut next_id = 1u32;
    for name in names {
        if name_to_id.contains_key(name) || name.is_empty() {
            continue;
        }
        let id = NetId(next_id);
        next_id += 1;
        name_to_id.insert(name.to_string(), id);
        board.nets.insert(
            id,
            Net {
                id,
                name: name.to_string(),
                net_class: "Default".to_string(),
            },
        );
    }
    name_to_id
}

/// Recursively collects every `(net "<name>")` reference's name (the
/// newer, ID-less shape only — a numeric `(net <id> ...)` is skipped) at
/// any depth in the document, in document order. See [`parse_nets`]'s
/// docs for why this needs a real recursive walk rather than
/// [`Sexpr::find_all`].
fn collect_name_only_net_refs<'a>(expr: &'a Sexpr, names: &mut Vec<&'a str>) {
    if expr.head() == Some("net") {
        if let Some(first) = expr.atom(1) {
            if first.parse::<u32>().is_err() {
                names.push(first);
            }
        }
    }
    if let Some(children) = expr.as_list() {
        for child in children {
            collect_name_only_net_refs(child, names);
        }
    }
}

/// Resolves one item's own `(net ...)` expression to a [`NetId`],
/// transparently handling both shapes [`parse_nets`] documents: a
/// numeric `(net <id> ...)` resolves directly (no map lookup needed); a
/// name-only `(net "<name>")` looks the name up in `name_to_id` (built
/// by `parse_nets` from the same file, so every name-only reference is
/// guaranteed present — `None` here would mean a genuinely malformed
/// file, not a normal case).
fn resolve_net_id(net_expr: &Sexpr, name_to_id: &HashMap<String, NetId>) -> Option<NetId> {
    if let Some(id) = net_expr.atom(1).and_then(|s| s.parse::<u32>().ok()) {
        return Some(NetId(id));
    }
    name_to_id.get(net_expr.atom(1)?).copied()
}

fn parse_one_track(
    seg: &Sexpr,
    id: TrackId,
    name_to_id: &HashMap<String, NetId>,
    layer_map: &HashMap<String, LayerId>,
) -> Option<Track> {
    let start = seg.find("start")?;
    let end = seg.find("end")?;
    let segment = Segment::new(
        Point::new(nm(start.atom(1)?)?, nm(start.atom(2)?)?),
        Point::new(nm(end.atom(1)?)?, nm(end.atom(2)?)?),
    );
    let width_nm = nm(seg.find("width")?.atom(1)?)?;
    let layer = resolve_layer_id(seg.find("layer")?.atom(1)?, layer_map)?;
    let net = resolve_net_id(seg.find("net")?, name_to_id)?;
    Some(Track {
        id,
        segment,
        width_nm,
        layer,
        net,
        locked: is_locked(seg),
    })
}

fn parse_tracks(
    root: &Sexpr,
    board: &mut Board,
    name_to_id: &HashMap<String, NetId>,
    layer_map: &HashMap<String, LayerId>,
    warnings: &mut Vec<String>,
) {
    for (i, seg) in root.find_all("segment").enumerate() {
        match parse_one_track(
            seg,
            TrackId(u32::try_from(i).unwrap_or(u32::MAX)),
            name_to_id,
            layer_map,
        ) {
            Some(track) => board.tracks.push(track),
            None => warnings.push(format!("skipped malformed or unsupported segment #{i}")),
        }
    }
}

fn parse_one_via(
    via: &Sexpr,
    id: ViaId,
    name_to_id: &HashMap<String, NetId>,
    layer_map: &HashMap<String, LayerId>,
) -> Option<Via> {
    let at = via.find("at")?;
    let position = Point::new(nm(at.atom(1)?)?, nm(at.atom(2)?)?);
    let diameter_nm = nm(via.find("size")?.atom(1)?)?;
    let drill_nm = nm(via.find("drill")?.atom(1)?)?;

    let layer_names: Vec<&str> = via
        .find("layers")?
        .as_list()?
        .iter()
        .skip(1)
        .filter_map(Sexpr::as_atom)
        .collect();
    if !layer_names.contains(&"F.Cu") || !layer_names.contains(&"B.Cu") {
        return None; // only through vias spanning both layers are supported
    }

    // A through via spans the *entire* stack, F.Cu to B.Cu, whatever
    // LayerIds those resolve to for this board's own layer count — not
    // hardcoded LayerId(0)/LayerId(1), which only happened to be right
    // for the 2-layer case.
    let from_layer = resolve_layer_id("F.Cu", layer_map)?;
    let to_layer = resolve_layer_id("B.Cu", layer_map)?;

    let net = resolve_net_id(via.find("net")?, name_to_id)?;
    Some(Via {
        id,
        position,
        diameter_nm,
        drill_nm,
        from_layer,
        to_layer,
        net,
        locked: is_locked(via),
    })
}

fn parse_vias(
    root: &Sexpr,
    board: &mut Board,
    name_to_id: &HashMap<String, NetId>,
    layer_map: &HashMap<String, LayerId>,
    warnings: &mut Vec<String>,
) {
    for (i, via) in root.find_all("via").enumerate() {
        match parse_one_via(
            via,
            ViaId(u32::try_from(i).unwrap_or(u32::MAX)),
            name_to_id,
            layer_map,
        ) {
            Some(v) => board.vias.push(v),
            None => warnings.push(format!("skipped malformed or unsupported via #{i}")),
        }
    }
}

/// Reads an `(at x y [ANGLE])` expression's rotation, rounded to the
/// nearest whole degree and normalized to `0..360`. `0` if there's no
/// third element at all (the overwhelmingly common unrotated case).
#[allow(clippy::cast_possible_truncation)]
fn angle_deg(at: &Sexpr) -> i64 {
    at.atom(3)
        .and_then(|s| s.parse::<f64>().ok())
        .map_or(0, |deg| deg.round() as i64)
        .rem_euclid(360)
}

/// Rotates `(dx, dy)` by `angle_deg` around the origin — exact integer
/// arithmetic, only for the four axis-preserving multiples of 90° this
/// crate models (see [`parse_one_pad`]'s docs for why other angles are
/// an honest skip rather than a float-approximated rotation). Verified
/// against real `kicad-cli`: a footprint at `(5, 5, 90)` with a pad at
/// local `(2, 1)` places that pad's absolute copper at `(6, 3)` in the
/// board's own file coordinates (Y increasing downward) — confirmed
/// directly by checking exactly that position (and no other candidate)
/// shorts against a marker pad on a different net, not derived from
/// unverified trigonometric convention guesses. See
/// `docs/DECISIONS.md`'s footprint-rotation entry for the fixtures.
fn rotate_90_multiple(dx: i64, dy: i64, angle_deg: i64) -> Option<(i64, i64)> {
    match angle_deg {
        0 => Some((dx, dy)),
        90 => Some((dy, -dx)),
        180 => Some((-dx, -dy)),
        270 => Some((-dy, dx)),
        _ => None, // not axis-aligned — not modelled, not guessed
    }
}

/// A pad's absolute centre: the footprint's rotation carries the pad's
/// *position* around the footprint's origin, whatever the pad's shape.
fn pad_position(pad: &Sexpr, footprint_at: Point, footprint_angle_deg: i64) -> Option<Point> {
    let local_at = pad.find("at")?;
    let local = Point::new(nm(local_at.atom(1)?)?, nm(local_at.atom(2)?)?);
    let (rx, ry) = rotate_90_multiple(local.x, local.y, footprint_angle_deg)?;
    Some(Point::new(footprint_at.x + rx, footprint_at.y + ry))
}

/// A non-plated pad's drilled hole. Only a plain circular `(drill D)` is
/// modelled; a slotted `(drill oval W H)` is skipped with a warning.
fn parse_npth_hole(
    pad: &Sexpr,
    footprint_at: Point,
    footprint_angle_deg: i64,
    id: HoleId,
) -> Option<Hole> {
    Some(Hole {
        id,
        position: pad_position(pad, footprint_at, footprint_angle_deg)?,
        diameter_nm: nm(pad.find("drill")?.atom(1)?)?,
    })
}

/// Verified against real `kicad-cli`: a non-plated pad whose size exceeds
/// its drill has real (no-net) copper around the hole; one whose size
/// equals the drill — the usual mounting hole — has none.
fn npth_has_copper(pad: &Sexpr) -> bool {
    let drill = pad.find("drill").and_then(|d| nm(d.atom(1)?));
    let size = pad.find("size");
    let dims = [1, 2].map(|i| size.and_then(|s| nm(s.atom(i)?)));
    match drill {
        Some(d) => dims.iter().flatten().any(|&w| w > d),
        None => false,
    }
}

fn parse_one_pad(
    pad: &Sexpr,
    footprint_at: Point,
    footprint_angle_deg: i64,
    id: PadId,
    name_to_id: &HashMap<String, NetId>,
    layer_map: &HashMap<String, LayerId>,
) -> Option<Pad> {
    // (pad "<number>" <pad_type e.g. smd/thru_hole> <shape e.g. circle/rect> ...)
    let shape_kind = pad.atom(3)?;
    let local_at = pad.find("at")?;
    let absolute = pad_position(pad, footprint_at, footprint_angle_deg)?;

    // In `.kicad_pcb` a pad's stored angle is its *absolute* orientation —
    // it already includes the footprint's rotation, so it must not be
    // composed with `footprint_angle_deg` again. Verified against real
    // `kicad-cli`: in a footprint at 90°, a `(size 2 0.5)` pad stored at
    // angle 0 measures horizontal, at 90 vertical, at 180 horizontal. See
    // `docs/DECISIONS.md`'s "Real-board pass, 2026-10-21" entry. Only
    // axis-preserving multiples of 90° are handled; any other angle is an
    // honest skip, not a mismeasured shape.
    let total_angle_deg = angle_deg(local_at);

    let shape = match shape_kind {
        "circle" => {
            let diameter_nm = nm(pad.find("size")?.atom(1)?)?;
            PadShape::Circle(Circle::new(absolute, diameter_nm / 2))
        }
        "rect" => {
            let size = pad.find("size")?;
            let width_nm = nm(size.atom(1)?)?;
            let height_nm = nm(size.atom(2)?)?;
            let (width_nm, height_nm) = match total_angle_deg {
                0 | 180 => (width_nm, height_nm),
                90 | 270 => (height_nm, width_nm),
                _ => return None, // not axis-aligned — not modelled, not guessed
            };
            PadShape::Rect(Rect::new(absolute, width_nm, height_nm))
        }
        "oval" => {
            let size = pad.find("size")?;
            let width_nm = nm(size.atom(1)?)?;
            let height_nm = nm(size.atom(2)?)?;
            // Same swap as `rect` above, and for the same reason:
            // `Oval::new` picks its spine axis from whichever of
            // width/height is larger, so a 90°/270° combined rotation
            // needs the values swapped first or it would (silently)
            // keep the *original*, unrotated spine orientation.
            let (width_nm, height_nm) = match total_angle_deg {
                0 | 180 => (width_nm, height_nm),
                90 | 270 => (height_nm, width_nm),
                _ => return None, // not axis-aligned — not modelled, not guessed
            };
            PadShape::Oval(Oval::new(absolute, width_nm, height_nm))
        }
        // roundrect/trapezoid/custom — no clearance predicate exists for
        // these yet (see tessera_model::PadShape's docs).
        _ => return None,
    };

    // A through-hole pad's `(drill D)` is its circular drill diameter — the
    // same shape KiCad's own `hole_size` DRC constraint measures for pads.
    // Only the plain single-number form is read (a two-number `(drill W
    // H)` is an oval/slotted drill, out of scope alongside this module's
    // existing circular-pad-shape-only drill simplification).
    let drill_nm = pad.find("drill").and_then(|d| nm(d.atom(1)?));

    let mut layers: Vec<LayerId> = pad
        .find("layers")?
        .as_list()?
        .iter()
        .skip(1)
        .filter_map(Sexpr::as_atom)
        .flat_map(|n| resolve_layer_ids(n, layer_map))
        .collect();
    if layers.is_empty() {
        return None;
    }
    layers.sort_unstable();
    layers.dedup();

    // A pad with no `(net ...)` (or net 0) is still copper — "no net",
    // which other copper must keep clear of (verified against real
    // `kicad-cli`), just never something to route to.
    let net = match pad.find("net") {
        None => NetId::NONE,
        Some(net_expr) => resolve_net_id(net_expr, name_to_id)?,
    };

    Some(Pad {
        id,
        shape,
        layers,
        net,
        locked: false,   // set from the owning footprint's lock state by the caller
        reference: None, // set from the owning footprint's reference by the caller
        number: pad.atom(1).map(str::to_string),
        drill_nm,
    })
}

/// The owning footprint's reference designator (e.g. `"IC94"`) — read
/// from its `(property "Reference" "IC94" ...)` block. There can be
/// several `(property "Name" "Value" ...)` entries per footprint
/// (Reference, Value, Footprint, Datasheet, ...); `find` alone would
/// return whichever is declared first, not necessarily Reference, so
/// this filters `find_all` by the property's own name instead of
/// assuming field order.
fn footprint_reference(footprint: &Sexpr) -> Option<String> {
    footprint
        .find_all("property")
        .find(|p| p.atom(1) == Some("Reference"))
        .and_then(|p| p.atom(2))
        .map(str::to_string)
}

fn parse_pads(
    root: &Sexpr,
    board: &mut Board,
    name_to_id: &HashMap<String, NetId>,
    layer_map: &HashMap<String, LayerId>,
    warnings: &mut Vec<String>,
) {
    let mut pad_index = 0u32;
    for footprint in root.find_all("footprint") {
        let Some((fp_at, fp_angle_deg)) = footprint
            .find("at")
            .and_then(|a| Some((Point::new(nm(a.atom(1)?)?, nm(a.atom(2)?)?), angle_deg(a))))
        else {
            warnings.push("skipped footprint with missing/invalid position".to_string());
            continue;
        };
        let fp_locked = is_locked(footprint);
        let fp_reference = footprint_reference(footprint);

        for pad in footprint
            .as_list()
            .unwrap_or(&[])
            .iter()
            .filter(|c| c.head() == Some("pad"))
        {
            let id = PadId(pad_index);
            pad_index += 1;
            if pad.atom(2) == Some("np_thru_hole") {
                let hole_id = HoleId(u32::try_from(board.npth_holes.len()).unwrap_or(u32::MAX));
                match parse_npth_hole(pad, fp_at, fp_angle_deg, hole_id) {
                    Some(hole) => board.npth_holes.push(hole),
                    None => warnings.push(format!(
                        "skipped unsupported non-plated hole (pad #{})",
                        id.0
                    )),
                }
                if !npth_has_copper(pad) {
                    continue;
                }
            }
            match parse_one_pad(pad, fp_at, fp_angle_deg, id, name_to_id, layer_map) {
                Some(mut p) => {
                    p.locked = fp_locked;
                    p.reference.clone_from(&fp_reference);
                    board.pads.push(p);
                }
                None => warnings.push(format!("skipped unsupported or malformed pad #{}", id.0)),
            }
        }
    }
}

/// True iff `zone` is a *named rule area* rather than an ordinary
/// copper-fill zone — real KiCad boards contain both under the same
/// `(zone ...)` tag, but only a zone with both `(name "...")` and
/// `(keepout ...)` clauses is one of plan §7.5.6's named rule areas
/// (verified against the real `underFPGA`/`underDDR` zones in
/// `vme-wren.kicad_pcb`, `docs/DECISIONS.md`'s "ADR-0002 addendum" entry
/// — plain net-connected copper-pour zones have neither clause). Ordinary
/// copper-pour zones aren't modelled by this crate at all yet, so they're
/// silently skipped here, not warned about — that's a distinct, deliberate
/// scope gap from a malformed rule area, which does warn.
fn is_named_rule_area(zone: &Sexpr) -> bool {
    zone.find("name").is_some() && zone.find("keepout").is_some()
}

fn parse_rule_areas(
    root: &Sexpr,
    board: &mut Board,
    layer_map: &HashMap<String, LayerId>,
    warnings: &mut Vec<String>,
) {
    let mut next_id = 0u32;
    for zone in root.find_all("zone").filter(|z| is_named_rule_area(z)) {
        match parse_one_rule_area(zone, RuleAreaId(next_id), layer_map) {
            Some(area) => {
                board.rule_areas.push(area);
                next_id += 1;
            }
            None => {
                warnings.push("skipped a malformed or unsupported rule-area zone".to_string());
            }
        }
    }
}

/// Every filled polygon of every copper zone (not keepout/rule areas).
/// Only straight-edged fills are read; a fill containing an arc is skipped
/// with a warning rather than approximated.
fn parse_zone_fills(
    root: &Sexpr,
    board: &mut Board,
    name_to_id: &HashMap<String, NetId>,
    layer_map: &HashMap<String, LayerId>,
    warnings: &mut Vec<String>,
) {
    let legacy_stroked_fills = root
        .find("version")
        .and_then(|v| v.atom(1)?.parse::<u32>().ok())
        .is_some_and(|v| v < 20_250_210);
    for zone in root
        .find_all("zone")
        .filter(|z| z.find("keepout").is_none())
    {
        let net = zone
            .find("net")
            .and_then(|n| resolve_net_id(n, name_to_id))
            .unwrap_or(NetId::NONE);
        let inflation_nm = if legacy_stroked_fills {
            zone.find("min_thickness")
                .and_then(|t| nm(t.atom(1)?))
                .unwrap_or(0)
                / 2
        } else {
            0
        };
        let mut zone_layers: Vec<LayerId> = zone
            .find("layers")
            .or_else(|| zone.find("layer"))
            .and_then(Sexpr::as_list)
            .unwrap_or(&[])
            .iter()
            .skip(1)
            .filter_map(Sexpr::as_atom)
            .flat_map(|n| resolve_layer_ids(n, layer_map))
            .collect();
        zone_layers.sort_unstable();
        zone_layers.dedup();
        board.copper_zones.push(CopperZone {
            net,
            layers: zone_layers,
        });
        let clearance_nm = zone
            .find("connect_pads")
            .and_then(|c| c.find("clearance"))
            .and_then(|c| nm(c.atom(1)?))
            .unwrap_or(0);
        for fill in zone.find_all("filled_polygon") {
            let Some(layer) = fill
                .find("layer")
                .and_then(|l| resolve_layer_id(l.atom(1)?, layer_map))
            else {
                continue; // a fill on a layer this crate doesn't model
            };
            match fill_vertices(fill) {
                Some(vertices) => board.zone_fills.push(ZoneFill {
                    net,
                    layer,
                    clearance_nm,
                    polygon: Polygon::new(vertices),
                    inflation_nm,
                }),
                None => {
                    warnings.push("skipped a zone fill with arcs or too few points".to_string());
                }
            }
        }
    }
}

fn fill_vertices(fill: &Sexpr) -> Option<Vec<Point>> {
    let vertices = fill
        .find("pts")?
        .as_list()?
        .iter()
        .skip(1)
        .map(|xy| {
            if xy.head() != Some("xy") {
                return None;
            }
            Some(Point::new(nm(xy.atom(1)?)?, nm(xy.atom(2)?)?))
        })
        .collect::<Option<Vec<Point>>>()?;
    (vertices.len() >= 3).then_some(vertices)
}

fn keepout_allows(keepout: &Sexpr, tag: &str) -> bool {
    keepout.find(tag).and_then(|k| k.atom(1)) == Some("allowed")
}

fn parse_one_rule_area(
    zone: &Sexpr,
    id: RuleAreaId,
    layer_map: &HashMap<String, LayerId>,
) -> Option<RuleArea> {
    let name = zone.find("name")?.atom(1)?.to_string();
    let keepout_expr = zone.find("keepout")?;
    let keepout = KeepoutFlags {
        tracks_allowed: keepout_allows(keepout_expr, "tracks"),
        vias_allowed: keepout_allows(keepout_expr, "vias"),
        pads_allowed: keepout_allows(keepout_expr, "pads"),
        copper_pour_allowed: keepout_allows(keepout_expr, "copperpour"),
        footprints_allowed: keepout_allows(keepout_expr, "footprints"),
    };

    // `(layers "F.Cu" "B.Cu" ...)` for a multi-layer area, `(layer
    // "F.Cu")` for a single-layer one — both forms appear in real boards.
    // A wildcard entry like `"*.Cu"` (meaning "every copper layer") is
    // expanded by `resolve_layer_ids`, same as for pads.
    let mut layers: Vec<LayerId> = zone
        .find("layers")
        .or_else(|| zone.find("layer"))
        .and_then(Sexpr::as_list)
        .unwrap_or(&[])
        .iter()
        .skip(1)
        .filter_map(Sexpr::as_atom)
        .flat_map(|n| resolve_layer_ids(n, layer_map))
        .collect();
    if layers.is_empty() {
        return None;
    }
    layers.sort_unstable();
    layers.dedup();

    let pts = zone.find("polygon")?.find("pts")?.as_list()?;
    let vertices: Vec<Point> = pts
        .iter()
        .skip(1) // the "pts" tag itself
        .filter_map(|xy| {
            let coords = xy.as_list()?;
            Some(Point::new(
                nm(coords.get(1)?.as_atom()?)?,
                nm(coords.get(2)?.as_atom()?)?,
            ))
        })
        .collect();
    if vertices.len() < 3 {
        return None;
    }

    Some(RuleArea {
        id,
        name,
        outline: Polygon::new(vertices),
        layers,
        keepout,
    })
}

/// The board's physical outline (`Edge.Cuts` layer), assembled from
/// top-level (not footprint-local — `Sexpr::find_all` is direct-children
/// only, so this never picks up a footprint's own courtyard/silkscreen
/// graphics) `gr_line`/`gr_rect` items into a single closed polygon.
///
/// `None` (with a warning explaining why) if: any `Edge.Cuts` graphic is a
/// `gr_arc` — this crate doesn't model arcs anywhere (see module docs),
/// and approximating one into a polyline would silently bake in exactly
/// the geometric inexactness this project's whole exact-integer-geometry
/// stance exists to avoid, not a case where guessing is acceptable; or the
/// collected line segments don't chain into exactly one closed loop
/// (a gap, a branch, or more than one disjoint outline). `None` with no
/// warning if the board simply has no `Edge.Cuts` graphics at all — that's
/// not a parse failure, just an unknown outline.
fn parse_board_outline(root: &Sexpr, warnings: &mut Vec<String>) -> Option<Polygon> {
    if root.find_all("gr_arc").any(is_on_edge_cuts) {
        warnings.push(
            "board outline includes an arc segment on Edge.Cuts — arcs aren't supported, no outline was built"
                .to_string(),
        );
        return None;
    }

    let mut segments = Vec::new();
    for line in root.find_all("gr_line").filter(|l| is_on_edge_cuts(l)) {
        let Some(segment) = parse_two_point_item(line) else {
            warnings.push("skipped a malformed Edge.Cuts gr_line".to_string());
            return None;
        };
        segments.push(segment);
    }
    for rect in root.find_all("gr_rect").filter(|r| is_on_edge_cuts(r)) {
        let Some(diagonal) = parse_two_point_item(rect) else {
            warnings.push("skipped a malformed Edge.Cuts gr_rect".to_string());
            return None;
        };
        segments.extend(rect_edges(diagonal));
    }

    if segments.is_empty() {
        return None; // no Edge.Cuts graphics at all — an unknown outline, not a parse failure
    }
    chain_into_closed_polygon(segments, warnings)
}

fn is_on_edge_cuts(item: &Sexpr) -> bool {
    item.find("layer").and_then(|l| l.atom(1)) == Some("Edge.Cuts")
}

/// Reads a `(start x y)`/`(end x y)` pair, shared by `gr_line` (a segment)
/// and `gr_rect` (two opposite corners of an axis-aligned rectangle).
fn parse_two_point_item(item: &Sexpr) -> Option<Segment> {
    let start = item.find("start")?;
    let end = item.find("end")?;
    Some(Segment::new(
        Point::new(nm(start.atom(1)?)?, nm(start.atom(2)?)?),
        Point::new(nm(end.atom(1)?)?, nm(end.atom(2)?)?),
    ))
}

/// The four edges of the axis-aligned rectangle whose opposite corners are
/// `diagonal`'s two endpoints.
fn rect_edges(diagonal: Segment) -> [Segment; 4] {
    let (p0, p1) = (diagonal.a, diagonal.b);
    let (top_right, bottom_left) = (Point::new(p1.x, p0.y), Point::new(p0.x, p1.y));
    [
        Segment::new(p0, top_right),
        Segment::new(top_right, p1),
        Segment::new(p1, bottom_left),
        Segment::new(bottom_left, p0),
    ]
}

/// Chains an unordered set of line segments into a single closed polygon
/// loop by matching shared endpoints, starting from the first segment.
/// Returns `None` (with a warning) if the segments don't form exactly one
/// closed loop using every single segment — a gap, a branch, or more than
/// one disjoint outline are all real possibilities in a hand-edited or
/// oddly-exported board that this deliberately doesn't guess through.
fn chain_into_closed_polygon(
    mut segments: Vec<Segment>,
    warnings: &mut Vec<String>,
) -> Option<Polygon> {
    let first = segments.remove(0);
    let start_point = first.a;
    let mut current = first.b;
    let mut vertices = vec![start_point];

    while current != start_point {
        let Some(pos) = segments
            .iter()
            .position(|s| s.a == current || s.b == current)
        else {
            warnings.push(
                "Edge.Cuts outline doesn't form a single closed loop — no outline was built"
                    .to_string(),
            );
            return None;
        };
        let seg = segments.remove(pos);
        let next = if seg.a == current { seg.b } else { seg.a };
        vertices.push(current);
        current = next;
    }

    if !segments.is_empty() {
        warnings.push(
            "board has more than one Edge.Cuts outline loop — only a single loop is supported, no outline was built"
                .to_string(),
        );
        return None;
    }
    if vertices.len() < 3 {
        warnings
            .push("Edge.Cuts outline has fewer than 3 vertices — no outline was built".to_string());
        return None;
    }

    Some(Polygon::new(vertices))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal 4-layer board using KiCad's real ordinal convention,
    // verified against a real 4-layer board (`example/esp32-plc.kicad_pcb`,
    // not committed to the corpus — see `docs/DECISIONS.md`'s
    // "Multi-layer parsing" entry): F.Cu is always ordinal 0, B.Cu is
    // always ordinal 2 regardless of layer count, and inner layers are
    // numbered 4, 6, 8, ... in physical stackup order — deliberately
    // *not* ascending numeric order (B.Cu's ordinal 2 sorts before the
    // inner layers' 4/6, even though B.Cu is physically last).
    const FOUR_LAYER_BOARD: &str = r#"
(kicad_pcb
	(version 20260206)
	(layers
		(0 "F.Cu" signal)
		(4 "In1.Cu" signal)
		(6 "In2.Cu" signal)
		(2 "B.Cu" signal)
		(5 "F.SilkS" user)
	)
	(net 0 "")
	(net 1 "SIG")
	(segment
		(start 0 0)
		(end 1 0)
		(width 0.2)
		(layer "In1.Cu")
		(net "SIG")
		(uuid "00000000-0000-0000-0000-000000000001")
	)
	(via
		(at 2 0)
		(size 0.5)
		(drill 0.25)
		(layers "F.Cu" "B.Cu")
		(net "SIG")
		(uuid "00000000-0000-0000-0000-000000000002")
	)
)
"#;

    #[test]
    fn registers_all_four_copper_layers_in_physical_stackup_order_not_numeric_ordinal_order() {
        let parsed = parse_board(FOUR_LAYER_BOARD, None).unwrap();
        let names: Vec<&str> = parsed
            .board
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"],
            "F.Cu first, B.Cu last, inner layers in between by their own ordinal — \
             not a plain ascending sort of KiCad's raw ordinals (0, 2, 4, 6), which \
             would wrongly put B.Cu (ordinal 2) second"
        );
    }

    #[test]
    fn a_track_on_an_inner_layer_resolves_to_that_layers_own_id() {
        let parsed = parse_board(FOUR_LAYER_BOARD, None).unwrap();
        assert_eq!(parsed.board.tracks.len(), 1);
        // In1.Cu is index 1 in stackup order (F.Cu=0, In1.Cu=1, ...).
        assert_eq!(parsed.board.tracks[0].layer, LayerId(1));
    }

    #[test]
    fn a_through_via_spans_the_full_four_layer_stack_not_hardcoded_layerid_0_and_1() {
        let parsed = parse_board(FOUR_LAYER_BOARD, None).unwrap();
        assert_eq!(parsed.board.vias.len(), 1);
        let via = &parsed.board.vias[0];
        assert_eq!(via.from_layer, LayerId(0), "F.Cu is stackup index 0");
        assert_eq!(
            via.to_layer,
            LayerId(3),
            "B.Cu is stackup index 3 on this 4-layer board, not the 2-layer case's LayerId(1)"
        );
    }

    // A minimal but structurally real board: one named rule-area zone
    // (mirroring vme-wren.kicad_pcb's real underFPGA zone shape, verified
    // this session — see docs/DECISIONS.md's "ADR-0002 addendum" entry)
    // with one keepout flag deliberately set to "not_allowed" rather than
    // every flag being "allowed", plus one ordinary copper-fill zone that
    // must NOT be mistaken for a rule area.
    const BOARD_WITH_A_RULE_AREA: &str = r#"
(kicad_pcb
	(version 20241229)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	(net 1 "GND")
	(zone
		(net 0)
		(net_name "")
		(layers "F.Cu" "B.Cu")
		(name "BuckStage")
		(keepout
			(tracks not_allowed)
			(vias allowed)
			(pads allowed)
			(copperpour allowed)
			(footprints allowed)
		)
		(polygon
			(pts
				(xy 1 1) (xy 5 1) (xy 5 5) (xy 1 5)
			)
		)
	)
	(zone
		(net 1)
		(net_name "GND")
		(layer "F.Cu")
		(polygon
			(pts
				(xy 0 0) (xy 20 0) (xy 20 20) (xy 0 20)
			)
		)
	)
)
"#;

    #[test]
    fn parses_a_named_rule_area_and_skips_an_ordinary_copper_zone() {
        let parsed = parse_board(BOARD_WITH_A_RULE_AREA, None).unwrap();
        assert_eq!(
            parsed.board.rule_areas.len(),
            1,
            "the ordinary copper-fill zone must not be counted as a rule area"
        );
        let area = &parsed.board.rule_areas[0];
        assert_eq!(area.name, "BuckStage");
        assert_eq!(area.layers, vec![LayerId(0), LayerId(1)]);
    }

    #[test]
    fn reads_mixed_keepout_flags_correctly() {
        let parsed = parse_board(BOARD_WITH_A_RULE_AREA, None).unwrap();
        let keepout = parsed.board.rule_areas[0].keepout;
        assert!(
            !keepout.tracks_allowed,
            "explicitly not_allowed in the fixture"
        );
        assert!(keepout.vias_allowed);
        assert!(keepout.pads_allowed);
        assert!(keepout.copper_pour_allowed);
        assert!(keepout.footprints_allowed);
    }

    #[test]
    fn rule_area_outline_matches_the_declared_polygon() {
        let parsed = parse_board(BOARD_WITH_A_RULE_AREA, None).unwrap();
        let area = &parsed.board.rule_areas[0];
        // 1mm..5mm square: its centre should read as inside, a point well
        // outside the polygon should not.
        assert!(area
            .outline
            .contains_point(Point::new(3_000_000, 3_000_000)));
        assert!(!area
            .outline
            .contains_point(Point::new(50_000_000, 50_000_000)));
    }

    #[test]
    fn a_named_zone_missing_keepout_is_not_treated_as_a_rule_area() {
        let text = r#"
(kicad_pcb
	(version 20241229)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	(zone
		(net 0)
		(net_name "")
		(layer "F.Cu")
		(name "NotActuallyARuleArea")
		(polygon
			(pts
				(xy 0 0) (xy 10 0) (xy 10 10) (xy 0 10)
			)
		)
	)
)
"#;
        let parsed = parse_board(text, None).unwrap();
        assert!(parsed.board.rule_areas.is_empty());
    }

    #[test]
    fn reads_the_owning_footprints_reference_and_the_pads_own_number() {
        // Mirrors the real (property "Reference" "IC94" ...) shape found
        // in vme-wren.kicad_pcb (docs/DECISIONS.md's "Wildcard/pairwise-
        // binding semantics" entry) — Value is declared before Reference
        // here deliberately, so a naive "first property wins" reader
        // would get this wrong.
        let text = r#"
(kicad_pcb
	(version 20241229)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	(net 1 "NET1")
	(footprint "wren:SOME_IC"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000001")
		(at 0 0)
		(property "Value" "SN74LVC1G"
			(at 0 1 0)
			(layer "F.Fab")
			(uuid "00000000-0000-0000-0000-000000000002")
			(effects (font (size 1 1)))
		)
		(property "Reference" "IC14"
			(at 0 -1 0)
			(layer "F.SilkS")
			(uuid "00000000-0000-0000-0000-000000000003")
			(effects (font (size 1 1)))
		)
		(attr smd)
		(pad "3" smd circle
			(at 0 0)
			(size 1 1)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000004")
		)
	)
)
"#;
        let parsed = parse_board(text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 1);
        let pad = &parsed.board.pads[0];
        assert_eq!(pad.reference.as_deref(), Some("IC14"));
        assert_eq!(pad.number.as_deref(), Some("3"));
    }

    #[test]
    fn reads_a_through_hole_pads_drill_diameter() {
        let text = r#"
(kicad_pcb
	(version 20241229)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	(net 1 "NET1")
	(footprint "wren:CONN"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000001")
		(at 0 0)
		(pad "1" thru_hole circle
			(at 0 0)
			(size 1.2 1.2)
			(drill 0.3)
			(layers "F.Cu" "B.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
	)
)
"#;
        let parsed = parse_board(text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 1);
        assert_eq!(parsed.board.pads[0].drill_nm, Some(300_000));
    }

    #[test]
    fn a_wildcard_star_cu_layer_expands_to_every_copper_layer_on_the_board() {
        // Verified against the real board this crate is tested against
        // (`example/esp32-plc.kicad_pcb`, kept local-only): 86 of its
        // through-hole pads — real connector/header pins, not decorative
        // — write `(layers "*.Cu" "*.Mask")` instead of naming each
        // copper layer, and were silently skipped entirely before
        // `resolve_layer_ids` existed (the literal string "*.Cu" matched
        // no real layer name, so the pad's whole `layers` list came back
        // empty).
        let text = r#"
(kicad_pcb
	(version 20241229)
	(layers
		(0 "F.Cu" signal)
		(4 "In1.Cu" signal)
		(6 "In2.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	(net 1 "NET1")
	(footprint "wren:CONN"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000001")
		(at 0 0)
		(pad "1" thru_hole circle
			(at 0 0)
			(size 1.2 1.2)
			(drill 0.3)
			(layers "*.Cu" "*.Mask")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
	)
)
"#;
        let parsed = parse_board(text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 1);
        let mut layers = parsed.board.pads[0].layers.clone();
        layers.sort_unstable();
        assert_eq!(
            layers,
            vec![LayerId(0), LayerId(1), LayerId(2), LayerId(3)],
            "*.Cu must expand to all four copper layers, and *.Mask (not \
             a copper layer this crate models) must simply contribute \
             nothing rather than being mismatched onto one"
        );
    }

    #[test]
    fn an_smd_pad_has_no_drill() {
        let text = r#"
(kicad_pcb
	(version 20241229)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	(net 1 "NET1")
	(footprint "wren:SOME_IC"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000001")
		(at 0 0)
		(attr smd)
		(pad "3" smd circle
			(at 0 0)
			(size 1 1)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000004")
		)
	)
)
"#;
        let parsed = parse_board(text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 1);
        assert_eq!(parsed.board.pads[0].drill_nm, None);
    }

    fn board_with_edge_cuts(edge_cuts_body: &str) -> String {
        format!(
            r#"
(kicad_pcb
	(version 20241229)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	{edge_cuts_body}
)
"#
        )
    }

    #[test]
    fn reads_a_rectangular_outline_from_four_gr_lines() {
        let text = board_with_edge_cuts(
            r#"
	(gr_line (start 0 0) (end 10 0) (layer "Edge.Cuts") (uuid "1"))
	(gr_line (start 10 0) (end 10 5) (layer "Edge.Cuts") (uuid "2"))
	(gr_line (start 10 5) (end 0 5) (layer "Edge.Cuts") (uuid "3"))
	(gr_line (start 0 5) (end 0 0) (layer "Edge.Cuts") (uuid "4"))
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        let outline = parsed.board.outline.expect("outline should be built");
        assert!(outline.contains_point(Point::new(5_000_000, 2_500_000)));
        assert!(!outline.contains_point(Point::new(50_000_000, 50_000_000)));
        assert!(parsed.warnings.is_empty());
    }

    #[test]
    fn reads_a_rectangular_outline_regardless_of_gr_line_declaration_order() {
        // Same rectangle as above, but the edges are declared out of
        // order and with mixed start/end direction — the chaining
        // algorithm must not depend on file order or edge direction.
        let text = board_with_edge_cuts(
            r#"
	(gr_line (start 10 5) (end 0 5) (layer "Edge.Cuts") (uuid "3"))
	(gr_line (start 0 0) (end 10 0) (layer "Edge.Cuts") (uuid "1"))
	(gr_line (start 0 0) (end 0 5) (layer "Edge.Cuts") (uuid "4"))
	(gr_line (start 10 0) (end 10 5) (layer "Edge.Cuts") (uuid "2"))
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        let outline = parsed.board.outline.expect("outline should be built");
        assert!(outline.contains_point(Point::new(5_000_000, 2_500_000)));
    }

    #[test]
    fn reads_a_rectangular_outline_from_a_single_gr_rect() {
        let text = board_with_edge_cuts(
            r#"
	(gr_rect (start 0 0) (end 10 5) (layer "Edge.Cuts") (uuid "1"))
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        let outline = parsed.board.outline.expect("outline should be built");
        assert!(outline.contains_point(Point::new(5_000_000, 2_500_000)));
        assert!(!outline.contains_point(Point::new(50_000_000, 50_000_000)));
    }

    #[test]
    fn an_arc_on_edge_cuts_produces_no_outline_and_a_warning() {
        let text = board_with_edge_cuts(
            r#"
	(gr_line (start 0 0) (end 10 0) (layer "Edge.Cuts") (uuid "1"))
	(gr_arc (start 10 0) (mid 12 2) (end 10 5) (layer "Edge.Cuts") (uuid "2"))
	(gr_line (start 10 5) (end 0 5) (layer "Edge.Cuts") (uuid "3"))
	(gr_line (start 0 5) (end 0 0) (layer "Edge.Cuts") (uuid "4"))
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert!(parsed.board.outline.is_none());
        assert!(parsed
            .warnings
            .iter()
            .any(|w| w.contains("arc") && w.contains("Edge.Cuts")));
    }

    #[test]
    fn an_open_outline_produces_no_outline_and_a_warning() {
        let text = board_with_edge_cuts(
            r#"
	(gr_line (start 0 0) (end 10 0) (layer "Edge.Cuts") (uuid "1"))
	(gr_line (start 10 0) (end 10 5) (layer "Edge.Cuts") (uuid "2"))
	(gr_line (start 10 5) (end 0 5) (layer "Edge.Cuts") (uuid "3"))
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert!(parsed.board.outline.is_none());
        assert!(parsed.warnings.iter().any(|w| w.contains("closed loop")));
    }

    #[test]
    fn a_board_with_no_edge_cuts_graphics_has_no_outline_and_no_warning() {
        let text = board_with_edge_cuts("");
        let parsed = parse_board(&text, None).unwrap();
        assert!(parsed.board.outline.is_none());
        assert!(parsed.warnings.is_empty());
    }

    #[test]
    fn reads_name_only_net_references_with_no_top_level_id_table() {
        // A newer .kicad_pcb shape, verified against a real user-supplied
        // board this crate hadn't seen before: no top-level (net <id>
        // "<name>") table at all, and every pad references its net
        // purely by name — (net "SIG"), no numeric ID.
        let text = r#"
(kicad_pcb
	(version 20260206)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(footprint "wren:SOME_IC"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000001")
		(at 0 0)
		(pad "1" smd circle
			(at 0 0)
			(size 1 1)
			(layers "F.Cu")
			(net "SIG")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
	)
	(footprint "wren:SOME_IC"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000003")
		(at 2 0)
		(pad "1" smd circle
			(at 0 0)
			(size 1 1)
			(layers "F.Cu")
			(net "SIG")
			(uuid "00000000-0000-0000-0000-000000000004")
		)
	)
)
"#;
        let parsed = parse_board(text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 2);
        assert_eq!(parsed.board.pads[0].net, parsed.board.pads[1].net);
        assert_eq!(parsed.board.nets.len(), 1);
        let net = parsed.board.nets.values().next().unwrap();
        assert_eq!(net.name, "SIG");
        assert!(parsed.warnings.is_empty());
    }

    #[test]
    fn name_only_net_references_assign_distinct_ids_to_distinct_names() {
        let text = r#"
(kicad_pcb
	(version 20260206)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(footprint "wren:SOME_IC"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000001")
		(at 0 0)
		(pad "1" smd circle
			(at 0 0)
			(size 1 1)
			(layers "F.Cu")
			(net "SIG_A")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
		(pad "2" smd circle
			(at 1 0)
			(size 1 1)
			(layers "F.Cu")
			(net "SIG_B")
			(uuid "00000000-0000-0000-0000-000000000003")
		)
	)
)
"#;
        let parsed = parse_board(text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 2);
        assert_ne!(parsed.board.pads[0].net, parsed.board.pads[1].net);
        assert_eq!(parsed.board.nets.len(), 2);
    }

    #[test]
    fn a_pad_with_no_net_field_at_all_is_still_skipped_in_the_name_only_shape() {
        // A mounting hole (no net at all, not even a placeholder name) —
        // must still be skipped, the same as the classic-format net-0
        // case, not crash or get an arbitrary net assigned.
        let text = r#"
(kicad_pcb
	(version 20260206)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(footprint "wren:MOUNTING_HOLE"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000001")
		(at 0 0)
		(pad "" np_thru_hole circle
			(at 0 0)
			(size 3 3)
			(drill 3)
			(layers "*.Cu" "*.Mask")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
	)
)
"#;
        let parsed = parse_board(text, None).unwrap();
        assert!(parsed.board.pads.is_empty());
    }

    fn board_with_one_pad(pad_body: &str) -> String {
        format!(
            r#"
(kicad_pcb
	(version 20241229)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	(net 1 "NET1")
	(footprint "wren:RECT"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000001")
		(at 0 0)
		{pad_body}
	)
)
"#
        )
    }

    fn board_with_zone(version: u32, fill_pts: &str) -> String {
        format!(
            r#"
(kicad_pcb
	(version {version})
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	(net 1 "NET1")
	(zone (net 1) (net_name "NET1") (layer "F.Cu")
		(connect_pads (clearance 0.25))
		(min_thickness 0.3)
		(polygon (pts (xy 0 0) (xy 5 0) (xy 5 5) (xy 0 5)))
		(filled_polygon (layer "F.Cu") (pts {fill_pts}))
	)
	(zone (net 0) (net_name "") (layers "F.Cu") (name "keepout")
		(keepout (tracks not_allowed) (vias allowed) (pads allowed) (copperpour allowed) (footprints allowed))
		(polygon (pts (xy 10 10) (xy 12 10) (xy 12 12)))
	)
)
"#
        )
    }

    #[test]
    fn reads_a_copper_zones_fill_but_not_a_keepout_area() {
        let text = board_with_zone(20_260_206, "(xy 0 0) (xy 5 0) (xy 5 5) (xy 0 5)");
        let parsed = parse_board(&text, None).unwrap();
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        let fills = &parsed.board.zone_fills;
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].net, NetId(1));
        assert_eq!(fills[0].layer, LayerId(0));
        assert_eq!(fills[0].clearance_nm, 250_000);
        assert_eq!(fills[0].inflation_nm, 0);
        assert_eq!(fills[0].polygon.vertices.len(), 4);
    }

    #[test]
    fn a_pre_20250210_file_grows_its_fills_by_half_the_min_thickness() {
        let text = board_with_zone(20_250_209, "(xy 0 0) (xy 5 0) (xy 5 5) (xy 0 5)");
        let fills = parse_board(&text, None).unwrap().board.zone_fills;
        assert_eq!(fills[0].inflation_nm, 150_000);
        let text = board_with_zone(20_250_210, "(xy 0 0) (xy 5 0) (xy 5 5) (xy 0 5)");
        assert_eq!(
            parse_board(&text, None).unwrap().board.zone_fills[0].inflation_nm,
            0
        );
    }

    #[test]
    fn a_zone_fill_with_an_arc_is_skipped_not_approximated() {
        let text = board_with_zone(
            20_260_206,
            "(xy 0 0) (arc (start 5 0) (mid 6 1) (end 5 2)) (xy 0 5)",
        );
        let parsed = parse_board(&text, None).unwrap();
        assert!(parsed.board.zone_fills.is_empty());
        assert_eq!(parsed.warnings.len(), 1);
    }

    #[test]
    fn a_mounting_hole_becomes_a_hole_with_no_copper() {
        let text = board_with_one_pad(
            r#"
		(pad "" np_thru_hole circle
			(at 1 2)
			(size 3.2 3.2)
			(drill 3.2)
			(layers "*.Cu" "*.Mask")
		)
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert!(parsed.board.pads.is_empty());
        assert_eq!(
            parsed.board.npth_holes,
            vec![Hole {
                id: HoleId(0),
                position: Point::new(1_000_000, 2_000_000),
                diameter_nm: 3_200_000,
            }]
        );
    }

    #[test]
    fn a_non_plated_pad_larger_than_its_drill_also_has_no_net_copper() {
        let text = board_with_one_pad(
            r#"
		(pad "" np_thru_hole circle
			(at 0 0)
			(size 3 3)
			(drill 2)
			(layers "*.Cu" "*.Mask")
		)
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert_eq!(parsed.board.npth_holes.len(), 1);
        assert_eq!(parsed.board.pads.len(), 1);
        assert_eq!(parsed.board.pads[0].net, NetId::NONE);
        assert_eq!(parsed.board.pads[0].layers, vec![LayerId(0), LayerId(1)]);
    }

    #[test]
    fn a_plated_pad_without_a_net_is_kept_as_no_net_copper() {
        let text = board_with_one_pad(
            r#"
		(pad "1" smd rect
			(at 0 0)
			(size 1 1)
			(layers "F.Cu")
		)
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.board.pads.len(), 1);
        assert_eq!(parsed.board.pads[0].net, NetId::NONE);
    }

    #[test]
    fn reads_board_design_rules_from_the_project_falling_back_to_kicads_defaults() {
        let text = board_with_one_pad("");
        let pro = r#"{"board":{"design_settings":{"rules":{"min_hole_clearance":0.2,"min_via_annular_width":0.15}}}}"#;
        let rules = parse_board(&text, Some(pro)).unwrap().board.rules;
        assert_eq!(rules.min_hole_clearance_nm, 200_000);
        assert_eq!(rules.min_via_annular_width_nm, 150_000);
        assert_eq!(
            rules.min_hole_to_hole_nm,
            DesignRules::default().min_hole_to_hole_nm
        );
    }

    #[test]
    fn reads_an_unrotated_rect_pad() {
        let text = board_with_one_pad(
            r#"
		(pad "1" smd rect
			(at 0 0)
			(size 2 1)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 1);
        let PadShape::Rect(r) = &parsed.board.pads[0].shape else {
            panic!("expected a rect pad");
        };
        assert_eq!(r.half_width_nm, 1_000_000);
        assert_eq!(r.half_height_nm, 500_000);
    }

    #[test]
    fn a_90_degree_rotated_rect_pad_swaps_width_and_height() {
        let text = board_with_one_pad(
            r#"
		(pad "1" smd rect
			(at 0 0 90)
			(size 2 1)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 1);
        let PadShape::Rect(r) = &parsed.board.pads[0].shape else {
            panic!("expected a rect pad");
        };
        // A 2x1mm pad rotated 90 degrees occupies 1x2mm in board space.
        assert_eq!(r.half_width_nm, 500_000);
        assert_eq!(r.half_height_nm, 1_000_000);
    }

    #[test]
    fn a_180_degree_rotated_rect_pad_keeps_width_and_height() {
        let text = board_with_one_pad(
            r#"
		(pad "1" smd rect
			(at 0 0 180)
			(size 2 1)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 1);
        let PadShape::Rect(r) = &parsed.board.pads[0].shape else {
            panic!("expected a rect pad");
        };
        assert_eq!(r.half_width_nm, 1_000_000);
        assert_eq!(r.half_height_nm, 500_000);
    }

    #[test]
    fn a_rect_pad_at_a_non_axis_aligned_angle_is_skipped_not_mismeasured() {
        let text = board_with_one_pad(
            r#"
		(pad "1" smd rect
			(at 0 0 45)
			(size 2 1)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert!(parsed.board.pads.is_empty());
        assert_eq!(parsed.warnings.len(), 1);
    }

    #[test]
    fn reads_an_unrotated_oval_pad() {
        let text = board_with_one_pad(
            r#"
		(pad "1" smd oval
			(at 0 0)
			(size 2 1)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 1);
        let PadShape::Oval(o) = &parsed.board.pads[0].shape else {
            panic!("expected an oval pad");
        };
        assert_eq!(o.radius_nm, 500_000);
        assert_eq!(o.spine.a, Point::new(-500_000, 0));
        assert_eq!(o.spine.b, Point::new(500_000, 0));
    }

    #[test]
    fn a_90_degree_rotated_oval_pad_swaps_its_spine_axis() {
        let text = board_with_one_pad(
            r#"
		(pad "1" smd oval
			(at 0 0 90)
			(size 2 1)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        let PadShape::Oval(o) = &parsed.board.pads[0].shape else {
            panic!("expected an oval pad");
        };
        assert_eq!(o.radius_nm, 500_000);
        assert_eq!(o.spine.a, Point::new(0, -500_000));
        assert_eq!(o.spine.b, Point::new(0, 500_000));
    }

    #[test]
    fn an_oval_pad_at_a_non_axis_aligned_angle_is_skipped_not_mismeasured() {
        let text = board_with_one_pad(
            r#"
		(pad "1" smd oval
			(at 0 0 45)
			(size 2 1)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert!(parsed.board.pads.is_empty());
        assert_eq!(parsed.warnings.len(), 1);
    }

    #[test]
    fn a_roundrect_pad_is_still_skipped_not_approximated() {
        let text = board_with_one_pad(
            r#"
		(pad "1" smd roundrect
			(at 0 0)
			(size 2 1)
			(roundrect_rratio 0.25)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
"#,
        );
        let parsed = parse_board(&text, None).unwrap();
        assert!(parsed.board.pads.is_empty());
    }

    /// Verified against real `kicad-cli`: a footprint at `(5, 5, 90)`
    /// with a pad at local `(2, 1)` places that pad's absolute copper
    /// at `(6, 3)` — see `rotate_90_multiple`'s docs.
    #[test]
    fn a_footprints_own_rotation_carries_its_pads_position() {
        let text = r#"
(kicad_pcb
	(version 20241229)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	(net 1 "NET1")
	(footprint "wren:RECT"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000001")
		(at 5 5 90)
		(pad "1" smd circle
			(at 2 1)
			(size 0.2 0.2)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
	)
)
"#;
        let parsed = parse_board(text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 1);
        let PadShape::Circle(c) = &parsed.board.pads[0].shape else {
            panic!("expected a circular pad");
        };
        assert_eq!(c.center, Point::new(6_000_000, 3_000_000));
    }

    fn rect_pad_in_a_90_degree_footprint(pad_at: &str) -> Rect {
        let text = format!(
            r#"
(kicad_pcb
	(version 20241229)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	(net 1 "NET1")
	(footprint "wren:RECT"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000001")
		(at 0 0 90)
		(pad "1" smd rect
			(at {pad_at})
			(size 2 1)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
	)
)
"#
        );
        let parsed = parse_board(&text, None).unwrap();
        assert_eq!(parsed.board.pads.len(), 1);
        let PadShape::Rect(r) = &parsed.board.pads[0].shape else {
            panic!("expected a rect pad");
        };
        *r
    }

    /// Verified against real `kicad-cli`: a pad's stored angle is its
    /// absolute orientation, already including the footprint's rotation —
    /// in a footprint at 90°, a pad stored at angle 0 is *not* rotated.
    #[test]
    fn a_pads_stored_angle_is_absolute_not_composed_with_the_footprints() {
        let unrotated = rect_pad_in_a_90_degree_footprint("0 0");
        assert_eq!(unrotated.half_width_nm, 1_000_000);
        assert_eq!(unrotated.half_height_nm, 500_000);

        let rotated = rect_pad_in_a_90_degree_footprint("0 0 90");
        assert_eq!(rotated.half_width_nm, 500_000);
        assert_eq!(rotated.half_height_nm, 1_000_000);
    }

    #[test]
    fn a_footprint_at_a_non_axis_aligned_angle_skips_every_one_of_its_pads() {
        // Even a circular pad (shape-wise rotation-invariant) can't be
        // placed correctly if the footprint's own rotation can't be
        // applied to its position exactly.
        let text = r#"
(kicad_pcb
	(version 20241229)
	(layers
		(0 "F.Cu" signal)
		(2 "B.Cu" signal)
	)
	(net 0 "")
	(net 1 "NET1")
	(footprint "wren:RECT"
		(layer "F.Cu")
		(uuid "00000000-0000-0000-0000-000000000001")
		(at 0 0 45)
		(pad "1" smd circle
			(at 2 0)
			(size 0.2 0.2)
			(layers "F.Cu")
			(net 1 "NET1")
			(uuid "00000000-0000-0000-0000-000000000002")
		)
	)
)
"#;
        let parsed = parse_board(text, None).unwrap();
        assert!(parsed.board.pads.is_empty());
        assert_eq!(parsed.warnings.len(), 1);
    }
}
