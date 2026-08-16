//! Semantic extraction of a [`Board`] from `.kicad_pcb` + optional
//! companion `.kicad_pro` text.
//!
//! Scope, matching [`crate::fixture`]'s writer so the two are symmetric for
//! round-trip testing: exactly two copper layers (`F.Cu`/`B.Cu`), straight
//! tracks only (no arcs), through vias only (no blind/buried), circular
//! pads only, and footprint rotation is **not** applied to pad positions
//! (a pad's absolute position is taken as its footprint's `at` plus the
//! pad's local `at`, assuming zero footprint rotation). Real boards using
//! any of these are only partially ingested — see [`ParsedBoard::warnings`]
//! for what got dropped, rather than silently losing fidelity with no
//! trace. Net class assignment is `"Default"` for every net; KiCad's
//! `netclass_patterns`/`netclass_assignments` matching isn't implemented.

use tessera_geom::{Circle, Point, Polygon, Segment};
use tessera_model::{
    Board, KeepoutFlags, Layer, LayerId, Net, NetClass, NetId, Pad, PadId, PadShape, RuleArea,
    RuleAreaId, Track, TrackId, Via, ViaId,
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
    #[error(
        "board does not have both F.Cu and B.Cu copper layers — only 2-layer boards are supported"
    )]
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
/// isn't `(kicad_pcb ...)`, it lacks both F.Cu and B.Cu copper layers (see
/// module docs for the 2-layer scope limit), or `pro_text` is present but
/// isn't valid JSON.
pub fn parse_board(pcb_text: &str, pro_text: Option<&str>) -> Result<ParsedBoard, ParseBoardError> {
    let root = sexpr::parse(pcb_text)?;
    if root.head() != Some("kicad_pcb") {
        return Err(ParseBoardError::NotAKicadPcb);
    }

    let mut warnings = Vec::new();
    let mut board = Board::new();

    parse_layers(&root, &mut board)?;
    parse_net_classes(pro_text, &mut board)?;
    parse_nets(&root, &mut board);
    parse_tracks(&root, &mut board, &mut warnings);
    parse_vias(&root, &mut board, &mut warnings);
    parse_pads(&root, &mut board, &mut warnings);
    parse_rule_areas(&root, &mut board, &mut warnings);
    board.outline = parse_board_outline(&root, &mut warnings);

    Ok(ParsedBoard { board, warnings })
}

fn layer_id_from_name(name: &str) -> Option<LayerId> {
    match name {
        "F.Cu" => Some(LayerId(0)),
        "B.Cu" => Some(LayerId(1)),
        _ => None,
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

fn parse_layers(root: &Sexpr, board: &mut Board) -> Result<(), ParseBoardError> {
    let layers_list = root
        .find("layers")
        .ok_or(ParseBoardError::MissingCopperLayers)?;
    let entries = layers_list.as_list().unwrap_or(&[]);
    let found_front = entries.iter().any(|e| e.atom(1) == Some("F.Cu"));
    let found_back = entries.iter().any(|e| e.atom(1) == Some("B.Cu"));
    if !found_front || !found_back {
        return Err(ParseBoardError::MissingCopperLayers);
    }
    board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
    board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
    Ok(())
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
    }
    board
        .net_classes
        .entry("Default".to_string())
        .or_insert_with(NetClass::default_placeholder);
    Ok(())
}

fn parse_nets(root: &Sexpr, board: &mut Board) {
    for net_entry in root.find_all("net") {
        let Some(id) = net_entry.atom(1).and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if id == 0 {
            continue; // net 0 is KiCad's "no net" sentinel.
        }
        let name = net_entry.atom(2).unwrap_or("").to_string();
        board.nets.insert(
            NetId(id),
            Net {
                id: NetId(id),
                name,
                net_class: "Default".to_string(),
            },
        );
    }
}

fn parse_one_track(seg: &Sexpr, id: TrackId) -> Option<Track> {
    let start = seg.find("start")?;
    let end = seg.find("end")?;
    let segment = Segment::new(
        Point::new(nm(start.atom(1)?)?, nm(start.atom(2)?)?),
        Point::new(nm(end.atom(1)?)?, nm(end.atom(2)?)?),
    );
    let width_nm = nm(seg.find("width")?.atom(1)?)?;
    let layer = layer_id_from_name(seg.find("layer")?.atom(1)?)?;
    let net = NetId(seg.find("net")?.atom(1)?.parse().ok()?);
    Some(Track {
        id,
        segment,
        width_nm,
        layer,
        net,
        locked: is_locked(seg),
    })
}

fn parse_tracks(root: &Sexpr, board: &mut Board, warnings: &mut Vec<String>) {
    for (i, seg) in root.find_all("segment").enumerate() {
        match parse_one_track(seg, TrackId(u32::try_from(i).unwrap_or(u32::MAX))) {
            Some(track) => board.tracks.push(track),
            None => warnings.push(format!("skipped malformed or unsupported segment #{i}")),
        }
    }
}

fn parse_one_via(via: &Sexpr, id: ViaId) -> Option<Via> {
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

    let net = NetId(via.find("net")?.atom(1)?.parse().ok()?);
    Some(Via {
        id,
        position,
        diameter_nm,
        drill_nm,
        from_layer: LayerId(0),
        to_layer: LayerId(1),
        net,
        locked: is_locked(via),
    })
}

fn parse_vias(root: &Sexpr, board: &mut Board, warnings: &mut Vec<String>) {
    for (i, via) in root.find_all("via").enumerate() {
        match parse_one_via(via, ViaId(u32::try_from(i).unwrap_or(u32::MAX))) {
            Some(v) => board.vias.push(v),
            None => warnings.push(format!("skipped malformed or unsupported via #{i}")),
        }
    }
}

fn parse_one_pad(pad: &Sexpr, footprint_at: Point, id: PadId) -> Option<Pad> {
    // (pad "<number>" <pad_type e.g. smd/thru_hole> <shape e.g. circle/rect> ...)
    if pad.atom(3) != Some("circle") {
        return None; // only circular pads are modelled — see module docs
    }
    let local_at = pad.find("at")?;
    let local = Point::new(nm(local_at.atom(1)?)?, nm(local_at.atom(2)?)?);
    let diameter_nm = nm(pad.find("size")?.atom(1)?)?;
    // A through-hole pad's `(drill D)` is its circular drill diameter — the
    // same shape KiCad's own `hole_size` DRC constraint measures for pads.
    // Only the plain single-number form is read (a two-number `(drill W
    // H)` is an oval/slotted drill, out of scope alongside this module's
    // existing circular-pad-shape-only simplification).
    let drill_nm = pad.find("drill").and_then(|d| nm(d.atom(1)?));

    let layers: Vec<LayerId> = pad
        .find("layers")?
        .as_list()?
        .iter()
        .skip(1)
        .filter_map(Sexpr::as_atom)
        .filter_map(layer_id_from_name)
        .collect();
    if layers.is_empty() {
        return None;
    }

    let net_id: u32 = pad.find("net")?.atom(1)?.parse().ok()?;
    if net_id == 0 {
        return None; // unconnected pad — not relevant to routing
    }

    Some(Pad {
        id,
        shape: PadShape::Circle(Circle::new(
            Point::new(footprint_at.x + local.x, footprint_at.y + local.y),
            diameter_nm / 2,
        )),
        layers,
        net: NetId(net_id),
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

fn parse_pads(root: &Sexpr, board: &mut Board, warnings: &mut Vec<String>) {
    let mut pad_index = 0u32;
    for footprint in root.find_all("footprint") {
        let Some(fp_at) = footprint
            .find("at")
            .and_then(|a| Some(Point::new(nm(a.atom(1)?)?, nm(a.atom(2)?)?)))
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
            match parse_one_pad(pad, fp_at, id) {
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

fn parse_rule_areas(root: &Sexpr, board: &mut Board, warnings: &mut Vec<String>) {
    let mut next_id = 0u32;
    for zone in root.find_all("zone").filter(|z| is_named_rule_area(z)) {
        match parse_one_rule_area(zone, RuleAreaId(next_id)) {
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

fn keepout_allows(keepout: &Sexpr, tag: &str) -> bool {
    keepout.find(tag).and_then(|k| k.atom(1)) == Some("allowed")
}

fn parse_one_rule_area(zone: &Sexpr, id: RuleAreaId) -> Option<RuleArea> {
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
    // Layer names this crate's 2-layer scope can't map (e.g. an inner
    // layer) are silently dropped from the list, same as every other
    // layer-set field this parser reads; see module docs for that limit.
    let layers: Vec<LayerId> = zone
        .find("layers")
        .or_else(|| zone.find("layer"))
        .and_then(Sexpr::as_list)
        .unwrap_or(&[])
        .iter()
        .skip(1)
        .filter_map(Sexpr::as_atom)
        .filter_map(layer_id_from_name)
        .collect();
    if layers.is_empty() {
        return None;
    }

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
}
