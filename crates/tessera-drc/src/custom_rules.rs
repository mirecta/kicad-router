//! Wires `tessera_io_kicad::dru_eval`'s pure evaluator to a real
//! [`Board`], checking board items against parsed `.kicad_dru`
//! [`DesignRules`].
//!
//! **Scoped to `track_width`/`disallow`/`length` (single-item or
//! single-connection constraints) and now `clearance` (the first
//! genuinely pairwise kind — see [`check_custom_clearance`], which
//! resolves each item pair via `dru_eval::resolve_constraint_pairwise`
//! before falling back to [`Board::resolved_clearance_nm`]).** Other
//! bound-shaped kinds (`hole_size`, `via_diameter`, `diff_pair_*`) still
//! need their own item-type mapping. Extending this module to those kinds
//! is real follow-up work, not a gap to silently paper over.
//!
//! Diff-pair partner lookup, rule-area intersection, and connection-
//! endpoint lookup (`fromTo`'s facts) are all derived fresh from `Board`
//! per item — see [`item_facts_for_net`].

use std::collections::HashSet;

use tessera_geom::Segment;
use tessera_io_kicad::dru::DesignRules;
use tessera_io_kicad::dru_eval::{self, EndpointFacts, ItemFacts};
use tessera_model::{Board, NetId, Pad};

use crate::violation::ItemRef;

/// Which side of a `(min ..)`/`(max ..)` bound a [`CustomRuleViolation`]
/// broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    Min,
    Max,
}

/// One item's measured value falling outside a resolved `.kicad_dru`
/// constraint's bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRuleViolation {
    pub item: ItemRef,
    pub rule_name: String,
    pub constraint_kind: String,
    pub bound: Bound,
    pub required_nm: i64,
    pub actual_nm: i64,
}

/// Checks every track's width against `design_rules`' `track_width`
/// constraints (resolved per-item via `dru_eval`'s last-wins selection —
/// see that module's docs). Tracks with no matching rule at all are
/// simply not reported, the same way a track with no clearance violation
/// isn't reported by [`crate::check_clearance`].
#[must_use]
pub fn check_track_width(board: &Board, design_rules: &DesignRules) -> Vec<CustomRuleViolation> {
    let mut violations = Vec::new();
    for track in &board.tracks {
        let areas = areas_intersecting_segment(board, track.segment);
        let facts = item_facts_for_net(board, track.net, &areas);
        let Some((rule, constraint)) =
            dru_eval::resolve_constraint(&design_rules.rules, &facts, "track_width")
        else {
            continue;
        };
        if let Some((bound, required_nm)) = bound_violated(track.width_nm, constraint) {
            violations.push(CustomRuleViolation {
                item: ItemRef::Track(track.id),
                rule_name: rule.name.clone(),
                constraint_kind: "track_width".to_string(),
                bound,
                required_nm,
                actual_nm: track.width_nm,
            });
        }
    }
    violations
}

/// One item whose type is listed in a matching `(constraint disallow
/// ...)` rule's item-type args.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisallowViolation {
    pub item: ItemRef,
    pub rule_name: String,
}

/// Checks every track and via against `design_rules`' `disallow`
/// constraints, using [`dru_eval::resolve_disallow`]'s item-type-scoped
/// last-wins resolution (**not** [`dru_eval::resolve_constraint`] — see
/// that function's docs for the empirically-verified reason a
/// kind-only-scoped last-wins would get this wrong). A via's rule-area
/// membership is approximated by its centre point only
/// (`RuleArea::outline.contains_point`) — this doesn't account for the
/// via's own diameter extending across a boundary it merely sits near,
/// unlike a track's `intersects_segment` check, which is exact.
#[must_use]
pub fn check_disallow(board: &Board, design_rules: &DesignRules) -> Vec<DisallowViolation> {
    let mut violations = Vec::new();
    for track in &board.tracks {
        let areas = areas_intersecting_segment(board, track.segment);
        let facts = item_facts_for_net(board, track.net, &areas);
        if let Some(rule) = dru_eval::resolve_disallow(&design_rules.rules, &facts, "track") {
            violations.push(DisallowViolation {
                item: ItemRef::Track(track.id),
                rule_name: rule.name.clone(),
            });
        }
    }
    for via in &board.vias {
        let areas = areas_containing_point(board, via.position);
        let facts = item_facts_for_net(board, via.net, &areas);
        if let Some(rule) = dru_eval::resolve_disallow(&design_rules.rules, &facts, "via") {
            violations.push(DisallowViolation {
                item: ItemRef::Via(via.id),
                rule_name: rule.name.clone(),
            });
        }
    }
    violations
}

fn areas_intersecting_segment(board: &Board, segment: tessera_geom::Segment) -> Vec<&str> {
    board
        .rule_areas
        .iter()
        .filter(|area| area.outline.intersects_segment(segment))
        .map(|area| area.name.as_str())
        .collect()
}

/// Point-membership area lookup — used for vias and pads, whose rule-area
/// membership is approximated by their centre point only (not their full
/// extent), unlike a track's exact `intersects_segment` check. Documented
/// as a known simplification the first time it was used
/// ([`check_disallow`]'s via handling); reused as-is here rather than
/// re-derived, not extended to be more exact.
fn areas_containing_point(board: &Board, point: tessera_geom::Point) -> Vec<&str> {
    board
        .rule_areas
        .iter()
        .filter(|area| area.outline.contains_point(point))
        .map(|area| area.name.as_str())
        .collect()
}

/// A connection's total routed length falling outside a resolved
/// `.kicad_dru` `length` constraint's bound. Unlike [`CustomRuleViolation`]
/// (scoped to one track/via/pad item), a length violation is inherently
/// about a whole connection — matching how real `kicad-cli` reports it
/// against the connection's two terminal pads, not any single track
/// segment (see [`check_length`]'s docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LengthViolation {
    pub net: NetId,
    pub rule_name: String,
    pub bound: Bound,
    pub required_nm: i64,
    pub actual_nm: i64,
}

/// Checks every fully-routed two-pin net's total track length against
/// `design_rules`' `length` constraints.
///
/// Two semantics here are empirically verified against real `kicad-cli`
/// (see `docs/DECISIONS.md`'s length-constraint-semantics entry), not
/// assumed by analogy with `track_width`:
///
/// - **`actual` is the sum of every track segment belonging to the
///   connection**, reported once per connection — not once per segment.
///   A two-segment 1mm+1mm connection under a `(max 1.5mm)` rule reports
///   a single violation with `actual = 2.0mm`, not two 1mm-segment
///   violations that individually pass.
/// - **`intersectsArea`/`insideArea` require *every* segment of the
///   connection to intersect the named area, not just one** — the
///   opposite of `track_width`'s per-item semantics, where the condition
///   is evaluated independently per track and each segment is checked (or
///   not) on its own. Verified with three fixtures: an area covering only
///   one of two segments (rule does not fire), an area covering the whole
///   connection (fires), and — to rule out a boundary-touch artefact — an
///   area covering only the second segment with a clean gap from the
///   first (does not fire). This is why `areas` for this check is built
///   as the *intersection* of each segment's own intersecting areas
///   ([`areas_intersecting_all`]), not the union `check_track_width` uses
///   for a single item.
///
/// Scoped to two-pin nets: unlike `track_width`/`disallow` (which check
/// one item's own property regardless of net topology), summing "every
/// track segment on this net" for a 3+-pin net wouldn't correspond to any
/// single `fromTo`-style connection KiCad's `length` constraint actually
/// measures — it would just be the net's total copper, a different
/// question. A net with no routed segments at all isn't checked either
/// (nothing to measure), the same way an unrouted net isn't reported by
/// other checks in this module.
#[must_use]
pub fn check_length(board: &Board, design_rules: &DesignRules) -> Vec<LengthViolation> {
    let mut violations = Vec::new();
    for net in board.nets.keys().copied() {
        if board.two_pin_net_endpoints(net).is_none() {
            continue; // multi-pin/unconnected nets — see this function's docs
        }
        let segments: Vec<Segment> = board
            .tracks
            .iter()
            .filter(|t| t.net == net)
            .map(|t| t.segment)
            .collect();
        if segments.is_empty() {
            continue;
        }

        let areas = areas_intersecting_all(board, &segments);
        let facts = item_facts_for_net(board, net, &areas);
        let Some((rule, constraint)) =
            dru_eval::resolve_constraint(&design_rules.rules, &facts, "length")
        else {
            continue;
        };

        let actual_nm: i64 = segments.iter().map(|s| s.length_nm()).sum();
        if let Some((bound, required_nm)) = bound_violated(actual_nm, constraint) {
            violations.push(LengthViolation {
                net,
                rule_name: rule.name.clone(),
                bound,
                required_nm,
                actual_nm,
            });
        }
    }
    violations
}

/// The named rule areas that **every** one of `segments` intersects (set
/// intersection across segments, not union) — see [`check_length`]'s docs
/// for why a connection-level `length` check needs this instead of
/// `areas_intersecting_segment`'s single-segment union.
fn areas_intersecting_all<'a>(board: &'a Board, segments: &[Segment]) -> Vec<&'a str> {
    let Some((first, rest)) = segments.split_first() else {
        return Vec::new();
    };
    let mut areas: HashSet<&str> = areas_intersecting_segment(board, *first)
        .into_iter()
        .collect();
    for segment in rest {
        let segment_areas: HashSet<&str> = areas_intersecting_segment(board, *segment)
            .into_iter()
            .collect();
        areas.retain(|area| segment_areas.contains(area));
    }
    areas.into_iter().collect()
}

fn bound_violated(
    actual_nm: i64,
    constraint: &tessera_io_kicad::dru::Constraint,
) -> Option<(Bound, i64)> {
    if let Some(min) = constraint.min_nm {
        if actual_nm < min {
            return Some((Bound::Min, min));
        }
    }
    if let Some(max) = constraint.max_nm {
        if actual_nm > max {
            return Some((Bound::Max, max));
        }
    }
    None
}

/// One pairwise clearance violation. Unlike [`crate::ClearanceViolation`]
/// (always the net-class-resolved value), `required_nm` here may come
/// from either a matching `.kicad_dru` `clearance` rule (`rule_name =
/// Some(..)`) or, if none matches this pair, the ordinary net-class
/// default (`rule_name = None`) — see [`check_custom_clearance`]'s docs
/// for why a matching custom rule *replaces* the net-class value rather
/// than combining with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomClearanceViolation {
    pub a: ItemRef,
    pub b: ItemRef,
    pub rule_name: Option<String>,
    pub required_nm: i64,
}

/// The clearance required between two items on `net_a`/`net_b`, resolving
/// a matching `.kicad_dru` `clearance` rule first and falling back to the
/// board's ordinary net-class-resolved clearance
/// ([`Board::resolved_clearance_nm`]) only if none matches (or a matching
/// rule has no `(min ..)` bound to offer).
///
/// **A matching custom rule replaces the net-class default outright — it
/// is not combined with it (not `max(custom, net_class)`).** Verified
/// directly against real `kicad-cli`: a custom rule specifying a *smaller*
/// clearance than the net-class default, matching a pair whose actual gap
/// sits between the two values, produces **no** violation — the smaller
/// custom bound is all that applied (see `docs/DECISIONS.md`'s
/// clearance-pairwise-semantics entry). This matches plan §3.1's stated
/// precedence chain directly: "board → net class → custom rule → local
/// override" — a custom rule outranks the net-class layer, it doesn't
/// merely tighten it.
fn resolved_pairwise_clearance_nm(
    board: &Board,
    design_rules: &DesignRules,
    net_a: NetId,
    net_b: NetId,
    facts_a: &ItemFacts,
    facts_b: &ItemFacts,
) -> Option<(Option<String>, i64)> {
    if net_a == net_b {
        return None;
    }
    if let Some((rule, constraint)) =
        dru_eval::resolve_constraint_pairwise(&design_rules.rules, facts_a, facts_b, "clearance")
    {
        if let Some(min_nm) = constraint.min_nm {
            return Some((Some(rule.name.clone()), min_nm));
        }
    }
    board
        .resolved_clearance_nm(net_a, net_b)
        .map(|nm| (None, nm))
}

/// All pairwise clearance violations on `board`, checked across the same
/// six item-pair types as [`crate::check_clearance`] (track/via/pad ×
/// track/via/pad) and using the exact same geometry/layer-sharing logic
/// (this module reuses `crate::clearance`'s `pub(crate)` helpers directly,
/// not a re-derived copy) — the only difference from the plain net-class
/// check is *what distance is required*, resolved per pair via
/// [`resolved_pairwise_clearance_nm`] instead of always
/// `Board::resolved_clearance_nm`.
///
/// A pad's rule-area membership is approximated by its centre point only
/// (via [`areas_containing_point`]), the same known simplification
/// `check_disallow` already has for vias — not re-derived to be exact
/// here either.
#[must_use]
pub fn check_custom_clearance(
    board: &Board,
    design_rules: &DesignRules,
) -> Vec<CustomClearanceViolation> {
    let mut violations = Vec::new();
    check_track_track_custom(board, design_rules, &mut violations);
    check_track_via_custom(board, design_rules, &mut violations);
    check_track_pad_custom(board, design_rules, &mut violations);
    check_via_via_custom(board, design_rules, &mut violations);
    check_via_pad_custom(board, design_rules, &mut violations);
    check_pad_pad_custom(board, design_rules, &mut violations);
    violations
}

fn check_track_track_custom(
    board: &Board,
    design_rules: &DesignRules,
    violations: &mut Vec<CustomClearanceViolation>,
) {
    for (i, a) in board.tracks.iter().enumerate() {
        for b in &board.tracks[i + 1..] {
            if a.layer != b.layer {
                continue;
            }
            let areas_a = areas_intersecting_segment(board, a.segment);
            let areas_b = areas_intersecting_segment(board, b.segment);
            let facts_a = item_facts_for_net(board, a.net, &areas_a);
            let facts_b = item_facts_for_net(board, b.net, &areas_b);
            let Some((rule_name, min_nm)) = resolved_pairwise_clearance_nm(
                board,
                design_rules,
                a.net,
                b.net,
                &facts_a,
                &facts_b,
            ) else {
                continue;
            };
            let effective_min = min_nm + a.width_nm / 2 + b.width_nm / 2;
            if !a.segment.clears_segment(b.segment, effective_min) {
                violations.push(CustomClearanceViolation {
                    a: ItemRef::Track(a.id),
                    b: ItemRef::Track(b.id),
                    rule_name,
                    required_nm: min_nm,
                });
            }
        }
    }
}

fn check_track_via_custom(
    board: &Board,
    design_rules: &DesignRules,
    violations: &mut Vec<CustomClearanceViolation>,
) {
    for t in &board.tracks {
        for v in &board.vias {
            if !crate::clearance::track_via_share_layer(t, v) {
                continue;
            }
            let areas_t = areas_intersecting_segment(board, t.segment);
            let areas_v = areas_containing_point(board, v.position);
            let facts_t = item_facts_for_net(board, t.net, &areas_t);
            let facts_v = item_facts_for_net(board, v.net, &areas_v);
            let Some((rule_name, min_nm)) = resolved_pairwise_clearance_nm(
                board,
                design_rules,
                t.net,
                v.net,
                &facts_t,
                &facts_v,
            ) else {
                continue;
            };
            let effective_min = min_nm + t.width_nm / 2;
            if !crate::clearance::via_circle(v).clears_segment(t.segment, effective_min) {
                violations.push(CustomClearanceViolation {
                    a: ItemRef::Track(t.id),
                    b: ItemRef::Via(v.id),
                    rule_name,
                    required_nm: min_nm,
                });
            }
        }
    }
}

fn check_track_pad_custom(
    board: &Board,
    design_rules: &DesignRules,
    violations: &mut Vec<CustomClearanceViolation>,
) {
    for t in &board.tracks {
        for p in &board.pads {
            if !crate::clearance::track_pad_share_layer(t, p) {
                continue;
            }
            let areas_t = areas_intersecting_segment(board, t.segment);
            let areas_p = areas_containing_point(board, crate::clearance::pad_circle(p).center);
            let facts_t = item_facts_for_net(board, t.net, &areas_t);
            let facts_p = item_facts_for_net(board, p.net, &areas_p);
            let Some((rule_name, min_nm)) = resolved_pairwise_clearance_nm(
                board,
                design_rules,
                t.net,
                p.net,
                &facts_t,
                &facts_p,
            ) else {
                continue;
            };
            let effective_min = min_nm + t.width_nm / 2;
            if !crate::clearance::pad_circle(p).clears_segment(t.segment, effective_min) {
                violations.push(CustomClearanceViolation {
                    a: ItemRef::Track(t.id),
                    b: ItemRef::Pad(p.id),
                    rule_name,
                    required_nm: min_nm,
                });
            }
        }
    }
}

fn check_via_via_custom(
    board: &Board,
    design_rules: &DesignRules,
    violations: &mut Vec<CustomClearanceViolation>,
) {
    for (i, a) in board.vias.iter().enumerate() {
        for b in &board.vias[i + 1..] {
            if !crate::clearance::via_via_share_layer(a, b) {
                continue;
            }
            let areas_a = areas_containing_point(board, a.position);
            let areas_b = areas_containing_point(board, b.position);
            let facts_a = item_facts_for_net(board, a.net, &areas_a);
            let facts_b = item_facts_for_net(board, b.net, &areas_b);
            let Some((rule_name, min_nm)) = resolved_pairwise_clearance_nm(
                board,
                design_rules,
                a.net,
                b.net,
                &facts_a,
                &facts_b,
            ) else {
                continue;
            };
            if !crate::clearance::via_circle(a)
                .clears_circle(crate::clearance::via_circle(b), min_nm)
            {
                violations.push(CustomClearanceViolation {
                    a: ItemRef::Via(a.id),
                    b: ItemRef::Via(b.id),
                    rule_name,
                    required_nm: min_nm,
                });
            }
        }
    }
}

fn check_via_pad_custom(
    board: &Board,
    design_rules: &DesignRules,
    violations: &mut Vec<CustomClearanceViolation>,
) {
    for v in &board.vias {
        for p in &board.pads {
            if !crate::clearance::via_pad_share_layer(v, p) {
                continue;
            }
            let areas_v = areas_containing_point(board, v.position);
            let areas_p = areas_containing_point(board, crate::clearance::pad_circle(p).center);
            let facts_v = item_facts_for_net(board, v.net, &areas_v);
            let facts_p = item_facts_for_net(board, p.net, &areas_p);
            let Some((rule_name, min_nm)) = resolved_pairwise_clearance_nm(
                board,
                design_rules,
                v.net,
                p.net,
                &facts_v,
                &facts_p,
            ) else {
                continue;
            };
            if !crate::clearance::via_circle(v)
                .clears_circle(crate::clearance::pad_circle(p), min_nm)
            {
                violations.push(CustomClearanceViolation {
                    a: ItemRef::Via(v.id),
                    b: ItemRef::Pad(p.id),
                    rule_name,
                    required_nm: min_nm,
                });
            }
        }
    }
}

fn check_pad_pad_custom(
    board: &Board,
    design_rules: &DesignRules,
    violations: &mut Vec<CustomClearanceViolation>,
) {
    for (i, a) in board.pads.iter().enumerate() {
        for b in &board.pads[i + 1..] {
            if !crate::clearance::pad_pad_share_layer(a, b) {
                continue;
            }
            let areas_a = areas_containing_point(board, crate::clearance::pad_circle(a).center);
            let areas_b = areas_containing_point(board, crate::clearance::pad_circle(b).center);
            let facts_a = item_facts_for_net(board, a.net, &areas_a);
            let facts_b = item_facts_for_net(board, b.net, &areas_b);
            let Some((rule_name, min_nm)) = resolved_pairwise_clearance_nm(
                board,
                design_rules,
                a.net,
                b.net,
                &facts_a,
                &facts_b,
            ) else {
                continue;
            };
            if !crate::clearance::pad_circle(a)
                .clears_circle(crate::clearance::pad_circle(b), min_nm)
            {
                violations.push(CustomClearanceViolation {
                    a: ItemRef::Pad(a.id),
                    b: ItemRef::Pad(b.id),
                    rule_name,
                    required_nm: min_nm,
                });
            }
        }
    }
}

/// Builds [`ItemFacts`] for `track` from `board`'s current state — net
/// class, net name, diff-pair partner (if any), and (for a 2-pin net
/// only — see [`Board::two_pin_net_endpoints`]'s docs) the connection's
/// two terminal pads' reference/number. `areas` (which rule areas the
/// track's own segment intersects) is computed by the caller and passed
/// in rather than computed here, since it's an owned `Vec` that needs to
/// outlive this function's return value — the caller's loop body is
/// exactly the right scope for that ownership, not this function.
fn item_facts_for_net<'a>(board: &'a Board, net: NetId, areas: &'a [&'a str]) -> ItemFacts<'a> {
    let net_data = board.nets.get(&net);
    let net_class = board.net_class_for(net).map(|c| c.name.as_str());
    let net_name = net_data.map(|n| n.name.as_str());
    let diff_pair_partner_net_name = net_name.and_then(|name| diff_pair_partner_name(board, name));

    ItemFacts {
        net_class,
        net_name,
        diff_pair_partner_net_name,
        areas,
        connection_endpoints: connection_endpoints_for(board, net),
    }
}

fn connection_endpoints_for(board: &Board, net: NetId) -> Option<[EndpointFacts<'_>; 2]> {
    let [a, b] = board.two_pin_net_endpoints(net)?;
    Some([endpoint_facts(a), endpoint_facts(b)])
}

fn endpoint_facts(pad: &Pad) -> EndpointFacts<'_> {
    EndpointFacts {
        reference: pad.reference.as_deref(),
        number: pad.number.as_deref(),
    }
}

const DIFF_PAIR_SUFFIXES: [(&str, &str); 2] = [("_P", "_N"), ("+", "-")];

/// Given `name` (a net name), finds a companion net on `board` with the
/// same base name and the opposite diff-pair suffix, if `name` itself has
/// a recognized suffix at all (`_P`/`_N` or `+`/`-` — the two conventions
/// verified against real `kicad-cli` in `docs/DECISIONS.md`'s "Wildcard/
/// pairwise-binding semantics" entry; `_PLUS`/`_MINUS` and others are not
/// recognized there and aren't guessed at here either).
fn diff_pair_partner_name<'a>(board: &'a Board, name: &str) -> Option<&'a str> {
    for (suffix, companion_suffix) in DIFF_PAIR_SUFFIXES {
        if let Some(base) = name.strip_suffix(suffix) {
            let companion = format!("{base}{companion_suffix}");
            return board
                .nets
                .values()
                .find(|n| n.name == companion)
                .map(|n| n.name.as_str());
        }
        if let Some(base) = name.strip_suffix(companion_suffix) {
            let companion = format!("{base}{suffix}");
            return board
                .nets
                .values()
                .find(|n| n.name == companion)
                .map(|n| n.name.as_str());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::{Circle, Point, Segment};
    use tessera_io_kicad::dru::{Constraint, Rule};
    use tessera_model::{Layer, LayerId, Net, NetClass, PadId, Track, TrackId};

    fn board_with_one_track(width_nm: i64) -> (Board, NetId) {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
        board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
        board
            .net_classes
            .insert("Default".to_string(), NetClass::default_placeholder());
        let net = NetId(1);
        board.nets.insert(
            net,
            Net {
                id: net,
                name: "SIG".to_string(),
                net_class: "Default".to_string(),
            },
        );
        board.pads.push(Pad {
            id: PadId(0),
            shape: tessera_model::PadShape::Circle(Circle::new(Point::new(0, 0), 200_000)),
            layers: vec![LayerId(0)],
            net,
            locked: false,
            reference: Some("IC1".to_string()),
            number: Some("1".to_string()),
        });
        board.pads.push(Pad {
            id: PadId(1),
            shape: tessera_model::PadShape::Circle(Circle::new(Point::new(2_000_000, 0), 200_000)),
            layers: vec![LayerId(0)],
            net,
            locked: false,
            reference: Some("IC2".to_string()),
            number: Some("1".to_string()),
        });
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(0, 0), Point::new(2_000_000, 0)),
            width_nm,
            layer: LayerId(0),
            net,
            locked: false,
        });
        (board, net)
    }

    fn rules_with_track_width(
        min_nm: Option<i64>,
        max_nm: Option<i64>,
        condition: &str,
    ) -> DesignRules {
        DesignRules {
            version: Some(1),
            rules: vec![Rule {
                name: "test_rule".to_string(),
                layer: Vec::new(),
                constraints: vec![Constraint {
                    kind: "track_width".to_string(),
                    min_nm,
                    max_nm,
                    opt_nm: None,
                    args: Vec::new(),
                }],
                condition: Some(condition.to_string()),
            }],
        }
    }

    #[test]
    fn flags_a_track_narrower_than_the_resolved_minimum() {
        let (board, _) = board_with_one_track(100_000);
        let rules = rules_with_track_width(Some(200_000), None, "A.NetClass == 'Default'");
        let violations = check_track_width(&board, &rules);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].bound, Bound::Min);
        assert_eq!(violations[0].required_nm, 200_000);
        assert_eq!(violations[0].actual_nm, 100_000);
        assert_eq!(violations[0].rule_name, "test_rule");
    }

    #[test]
    fn does_not_flag_a_track_within_bounds() {
        let (board, _) = board_with_one_track(250_000);
        let rules = rules_with_track_width(Some(200_000), Some(300_000), "A.NetClass == 'Default'");
        assert!(check_track_width(&board, &rules).is_empty());
    }

    #[test]
    fn condition_gates_which_tracks_are_checked() {
        let (board, _) = board_with_one_track(100_000);
        let rules = rules_with_track_width(Some(200_000), None, "A.NetClass == 'SomeOtherClass'");
        assert!(check_track_width(&board, &rules).is_empty());
    }

    #[test]
    fn from_to_condition_matches_via_the_connections_terminal_pads() {
        let (board, _) = board_with_one_track(100_000);
        let rules = rules_with_track_width(Some(200_000), None, "A.fromTo('IC1-*','IC2-*')");
        let violations = check_track_width(&board, &rules);
        assert_eq!(
            violations.len(),
            1,
            "fromTo should match this track's own connection"
        );
    }

    #[test]
    fn from_to_condition_does_not_match_a_different_connection() {
        let (board, _) = board_with_one_track(100_000);
        let rules = rules_with_track_width(Some(200_000), None, "A.fromTo('ICX-*','ICY-*')");
        assert!(check_track_width(&board, &rules).is_empty());
    }

    #[test]
    fn intersects_area_condition_uses_the_tracks_own_geometry() {
        let (mut board, _) = board_with_one_track(100_000);
        board.rule_areas.push(tessera_model::RuleArea {
            id: tessera_model::RuleAreaId(0),
            name: "Zone1".to_string(),
            outline: tessera_geom::Polygon::new(vec![
                Point::new(-1_000_000, -1_000_000),
                Point::new(1_000_000, -1_000_000),
                Point::new(1_000_000, 1_000_000),
                Point::new(-1_000_000, 1_000_000),
            ]),
            layers: vec![LayerId(0)],
            keepout: tessera_model::KeepoutFlags {
                tracks_allowed: true,
                vias_allowed: true,
                pads_allowed: true,
                copper_pour_allowed: true,
                footprints_allowed: true,
            },
        });
        let rules = rules_with_track_width(Some(200_000), None, "A.intersectsArea('Zone1')");
        let violations = check_track_width(&board, &rules);
        assert_eq!(violations.len(), 1, "the track crosses through Zone1");
    }

    #[test]
    fn diff_pair_condition_finds_the_real_companion_net() {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
        board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
        board
            .net_classes
            .insert("Default".to_string(), NetClass::default_placeholder());
        let net_p = NetId(1);
        let net_n = NetId(2);
        board.nets.insert(
            net_p,
            Net {
                id: net_p,
                name: "SIG_P".to_string(),
                net_class: "Default".to_string(),
            },
        );
        board.nets.insert(
            net_n,
            Net {
                id: net_n,
                name: "SIG_N".to_string(),
                net_class: "Default".to_string(),
            },
        );
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(0, 0), Point::new(2_000_000, 0)),
            width_nm: 100_000,
            layer: LayerId(0),
            net: net_p,
            locked: false,
        });
        let rules = rules_with_track_width(Some(200_000), None, "A.inDiffPair('SIG')");
        let violations = check_track_width(&board, &rules);
        assert_eq!(violations.len(), 1);
    }

    fn rules_with_disallow(item_types: &[&str], condition: &str) -> DesignRules {
        DesignRules {
            version: Some(1),
            rules: vec![Rule {
                name: "test_rule".to_string(),
                layer: Vec::new(),
                constraints: vec![Constraint {
                    kind: "disallow".to_string(),
                    min_nm: None,
                    max_nm: None,
                    opt_nm: None,
                    args: item_types.iter().map(ToString::to_string).collect(),
                }],
                condition: Some(condition.to_string()),
            }],
        }
    }

    #[test]
    fn flags_a_track_whose_type_is_disallowed_and_condition_matches() {
        let (board, _) = board_with_one_track(100_000);
        let rules = rules_with_disallow(&["track"], "A.NetClass == 'Default'");
        let violations = check_disallow(&board, &rules);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].item, ItemRef::Track(TrackId(0)));
        assert_eq!(violations[0].rule_name, "test_rule");
    }

    #[test]
    fn does_not_flag_a_track_when_disallow_lists_a_different_item_type() {
        let (board, _) = board_with_one_track(100_000);
        let rules = rules_with_disallow(&["via"], "A.NetClass == 'Default'");
        assert!(check_disallow(&board, &rules).is_empty());
    }

    #[test]
    fn does_not_flag_a_track_when_condition_does_not_match() {
        let (board, _) = board_with_one_track(100_000);
        let rules = rules_with_disallow(&["track"], "A.NetClass == 'SomeOtherClass'");
        assert!(check_disallow(&board, &rules).is_empty());
    }

    #[test]
    fn flags_a_via_whose_type_is_disallowed_via_centre_point_area_membership() {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
        board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
        board
            .net_classes
            .insert("Default".to_string(), NetClass::default_placeholder());
        let net = NetId(1);
        board.nets.insert(
            net,
            Net {
                id: net,
                name: "SIG".to_string(),
                net_class: "Default".to_string(),
            },
        );
        board.vias.push(tessera_model::Via {
            id: tessera_model::ViaId(0),
            position: Point::new(0, 0),
            diameter_nm: 500_000,
            drill_nm: 250_000,
            from_layer: LayerId(0),
            to_layer: LayerId(1),
            net,
            locked: false,
        });
        board.rule_areas.push(tessera_model::RuleArea {
            id: tessera_model::RuleAreaId(0),
            name: "Zone1".to_string(),
            outline: tessera_geom::Polygon::new(vec![
                Point::new(-1_000_000, -1_000_000),
                Point::new(1_000_000, -1_000_000),
                Point::new(1_000_000, 1_000_000),
                Point::new(-1_000_000, 1_000_000),
            ]),
            layers: vec![LayerId(0), LayerId(1)],
            keepout: tessera_model::KeepoutFlags {
                tracks_allowed: true,
                vias_allowed: true,
                pads_allowed: true,
                copper_pour_allowed: true,
                footprints_allowed: true,
            },
        });

        let rules = rules_with_disallow(&["via"], "A.intersectsArea('Zone1')");
        let violations = check_disallow(&board, &rules);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].item, ItemRef::Via(tessera_model::ViaId(0)));
    }

    #[test]
    fn a_later_rule_for_a_different_item_type_does_not_suppress_the_real_violation() {
        // Verified against real kicad-cli (docs/DECISIONS.md's "First
        // real .kicad_dru violation" entry): resolve_disallow must scope
        // last-wins competition by item type, not by constraint kind
        // alone.
        let (board, _) = board_with_one_track(100_000);
        let mut rules = rules_with_disallow(&["track"], "A.NetClass == 'Default'");
        rules.rules[0].name = "disallow_track".to_string();
        rules.rules.push(Rule {
            name: "disallow_via_only".to_string(),
            layer: Vec::new(),
            constraints: vec![Constraint {
                kind: "disallow".to_string(),
                min_nm: None,
                max_nm: None,
                opt_nm: None,
                args: vec!["via".to_string()],
            }],
            condition: Some("A.NetClass == 'Default'".to_string()),
        });

        let violations = check_disallow(&board, &rules);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule_name, "disallow_track");
    }

    /// Two 1mm segments (0,0)->(1mm,0)->(2mm,0) between two pads — the
    /// exact geometry used in the real `kicad-cli` experiments recorded in
    /// `docs/DECISIONS.md`'s length-constraint-semantics entry.
    fn board_with_two_segment_track() -> (Board, NetId) {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
        board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
        board
            .net_classes
            .insert("Default".to_string(), NetClass::default_placeholder());
        let net = NetId(1);
        board.nets.insert(
            net,
            Net {
                id: net,
                name: "SIG".to_string(),
                net_class: "Default".to_string(),
            },
        );
        board.pads.push(Pad {
            id: PadId(0),
            shape: tessera_model::PadShape::Circle(Circle::new(Point::new(0, 0), 200_000)),
            layers: vec![LayerId(0)],
            net,
            locked: false,
            reference: Some("IC1".to_string()),
            number: Some("1".to_string()),
        });
        board.pads.push(Pad {
            id: PadId(1),
            shape: tessera_model::PadShape::Circle(Circle::new(Point::new(2_000_000, 0), 200_000)),
            layers: vec![LayerId(0)],
            net,
            locked: false,
            reference: Some("IC2".to_string()),
            number: Some("1".to_string()),
        });
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(0, 0), Point::new(1_000_000, 0)),
            width_nm: 200_000,
            layer: LayerId(0),
            net,
            locked: false,
        });
        board.tracks.push(Track {
            id: TrackId(1),
            segment: Segment::new(Point::new(1_000_000, 0), Point::new(2_000_000, 0)),
            width_nm: 200_000,
            layer: LayerId(0),
            net,
            locked: false,
        });
        (board, net)
    }

    fn rules_with_length(max_nm: i64, condition: &str) -> DesignRules {
        DesignRules {
            version: Some(1),
            rules: vec![Rule {
                name: "length_test".to_string(),
                layer: Vec::new(),
                constraints: vec![Constraint {
                    kind: "length".to_string(),
                    min_nm: None,
                    max_nm: Some(max_nm),
                    opt_nm: None,
                    args: Vec::new(),
                }],
                condition: Some(condition.to_string()),
            }],
        }
    }

    #[test]
    fn length_sums_every_segment_not_just_one() {
        // Verified against real kicad-cli: two 1mm segments under a
        // 1.5mm max bound report ONE violation with actual = 2.0mm (the
        // sum), not two passing 1mm-segment checks.
        let (board, net) = board_with_two_segment_track();
        let rules = rules_with_length(1_500_000, "A.fromTo('IC1-*','IC2-*')");
        let violations = check_length(&board, &rules);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].net, net);
        assert_eq!(violations[0].actual_nm, 2_000_000);
        assert_eq!(violations[0].required_nm, 1_500_000);
        assert_eq!(violations[0].bound, Bound::Max);
    }

    #[test]
    fn length_within_bound_is_not_flagged() {
        let (board, _) = board_with_two_segment_track();
        let rules = rules_with_length(3_000_000, "A.fromTo('IC1-*','IC2-*')");
        assert!(check_length(&board, &rules).is_empty());
    }

    fn rule_area(name: &str, min_x: i64, max_x: i64) -> tessera_model::RuleArea {
        tessera_model::RuleArea {
            id: tessera_model::RuleAreaId(0),
            name: name.to_string(),
            outline: tessera_geom::Polygon::new(vec![
                Point::new(min_x, -500_000),
                Point::new(max_x, -500_000),
                Point::new(max_x, 500_000),
                Point::new(min_x, 500_000),
            ]),
            layers: vec![LayerId(0)],
            keepout: tessera_model::KeepoutFlags {
                tracks_allowed: true,
                vias_allowed: true,
                pads_allowed: true,
                copper_pour_allowed: true,
                footprints_allowed: true,
            },
        }
    }

    #[test]
    fn length_intersects_area_requires_every_segment_not_just_one() {
        // Verified against real kicad-cli: an area covering only the
        // first of two segments does NOT satisfy intersectsArea for a
        // length constraint (opposite of track_width's per-item OR
        // semantics), but an area covering both segments does.
        let (board, _) = board_with_two_segment_track();
        let rules = rules_with_length(
            1_500_000,
            "A.fromTo('IC1-*','IC2-*') && A.intersectsArea('Zone1')",
        );

        let mut only_first = board.clone();
        only_first
            .rule_areas
            .push(rule_area("Zone1", -500_000, 500_000));
        assert!(
            check_length(&only_first, &rules).is_empty(),
            "area covering only one of two segments must not satisfy the condition"
        );

        let mut whole_connection = board;
        whole_connection
            .rule_areas
            .push(rule_area("Zone1", -500_000, 2_500_000));
        assert_eq!(
            check_length(&whole_connection, &rules).len(),
            1,
            "area covering every segment must satisfy the condition"
        );
    }

    #[test]
    fn length_skips_nets_with_no_routed_segments() {
        let (mut board, net) = board_with_two_segment_track();
        board.tracks.clear();
        let rules = rules_with_length(1, "A.fromTo('IC1-*','IC2-*')");
        assert!(check_length(&board, &rules).is_empty());
        let _ = net;
    }

    #[test]
    fn length_skips_multi_pin_nets() {
        let (mut board, net) = board_with_two_segment_track();
        board.pads.push(Pad {
            id: PadId(2),
            shape: tessera_model::PadShape::Circle(Circle::new(
                Point::new(1_000_000, 1_000_000),
                200_000,
            )),
            layers: vec![LayerId(0)],
            net,
            locked: false,
            reference: Some("IC3".to_string()),
            number: Some("1".to_string()),
        });
        let rules = rules_with_length(1, "A.NetClass == 'Default'");
        assert!(check_length(&board, &rules).is_empty());
    }

    fn two_class_board(hv_clearance_nm: i64, lv_clearance_nm: i64) -> (Board, NetId, NetId) {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
        board.layers.push(Layer::copper(LayerId(1), "B.Cu"));

        let mut hv = NetClass::default_placeholder();
        hv.name = "HV".to_string();
        hv.clearance_nm = hv_clearance_nm;
        board.net_classes.insert("HV".to_string(), hv);
        let mut lv = NetClass::default_placeholder();
        lv.name = "LV".to_string();
        lv.clearance_nm = lv_clearance_nm;
        board.net_classes.insert("LV".to_string(), lv);

        let net_x = NetId(1);
        let net_y = NetId(2);
        board.nets.insert(
            net_x,
            Net {
                id: net_x,
                name: "HVNET".to_string(),
                net_class: "HV".to_string(),
            },
        );
        board.nets.insert(
            net_y,
            Net {
                id: net_y,
                name: "LVNET".to_string(),
                net_class: "LV".to_string(),
            },
        );
        (board, net_x, net_y)
    }

    fn track_on(id: u32, net: NetId, y: i64) -> Track {
        Track {
            id: TrackId(id),
            segment: Segment::new(Point::new(0, y), Point::new(2_000_000, y)),
            width_nm: 200_000,
            layer: LayerId(0),
            net,
            locked: false,
        }
    }

    fn clearance_rule(name: &str, min_nm: i64, condition: &str) -> Rule {
        Rule {
            name: name.to_string(),
            layer: Vec::new(),
            constraints: vec![Constraint {
                kind: "clearance".to_string(),
                min_nm: Some(min_nm),
                max_nm: None,
                opt_nm: None,
                args: Vec::new(),
            }],
            condition: Some(condition.to_string()),
        }
    }

    fn design_rules_with(rules: Vec<Rule>) -> DesignRules {
        DesignRules {
            version: Some(1),
            rules,
        }
    }

    // Two tracks with centres 300_000nm apart, both width 200_000nm (half-
    // width 100_000nm each): actual edge-to-edge gap is 100_000nm.
    const PAIR_GAP_NM: i64 = 100_000;

    #[test]
    fn custom_clearance_matches_regardless_of_declared_order() {
        let (mut board, net_x, net_y) = two_class_board(50_000, 50_000);
        board.tracks.push(track_on(0, net_x, 0));
        board.tracks.push(track_on(1, net_y, 300_000));

        let forward = design_rules_with(vec![clearance_rule(
            "hv_lv",
            2_000_000,
            "A.NetClass == 'HV' && B.NetClass == 'LV'",
        )]);
        let violations = check_custom_clearance(&board, &forward);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule_name.as_deref(), Some("hv_lv"));
        assert_eq!(violations[0].required_nm, 2_000_000);

        // Verified against real kicad-cli: the reversed condition matches
        // the same physical pair.
        let reversed = design_rules_with(vec![clearance_rule(
            "lv_hv",
            2_000_000,
            "A.NetClass == 'LV' && B.NetClass == 'HV'",
        )]);
        assert_eq!(check_custom_clearance(&board, &reversed).len(), 1);
    }

    #[test]
    fn custom_clearance_replaces_rather_than_combines_with_net_class_default() {
        // Verified against real kicad-cli: net-class default is huge
        // (5mm), custom rule is small (50_000nm) and matches — the small
        // bound applies outright, not max(5mm, 50_000nm).
        let (mut board, net_x, net_y) = two_class_board(5_000_000, 5_000_000);
        board.tracks.push(track_on(0, net_x, 0));
        board.tracks.push(track_on(1, net_y, 300_000));

        let rules = design_rules_with(vec![clearance_rule(
            "small_override",
            50_000,
            "A.NetClass == 'HV'",
        )]);
        assert!(
            check_custom_clearance(&board, &rules).is_empty(),
            "gap {PAIR_GAP_NM}nm satisfies the 50_000nm custom bound, despite the 5mm net-class default"
        );
    }

    #[test]
    fn custom_clearance_falls_back_to_net_class_default_when_no_rule_matches() {
        let (mut board, net_x, net_y) = two_class_board(200_000, 200_000);
        board.tracks.push(track_on(0, net_x, 0));
        board.tracks.push(track_on(1, net_y, 300_000));

        let rules = design_rules_with(vec![clearance_rule(
            "irrelevant",
            50_000,
            "A.NetClass == 'SomethingElse'",
        )]);
        let violations = check_custom_clearance(&board, &rules);
        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].rule_name, None,
            "no custom rule matched, so the net-class default applied"
        );
        assert_eq!(violations[0].required_nm, 200_000);
    }

    #[test]
    fn custom_clearance_last_wins_for_competing_rules() {
        // Verified against real kicad-cli: same as track_width, not
        // disallow's item-type-scoped variant (clearance has no args list).
        let (mut board, net_x, net_y) = two_class_board(50_000, 50_000);
        board.tracks.push(track_on(0, net_x, 0));
        board.tracks.push(track_on(1, net_y, 300_000));

        let rules = design_rules_with(vec![
            clearance_rule("first_small", 50_000, "A.NetClass == 'HV'"),
            clearance_rule("second_large", 200_000, "A.NetClass == 'HV'"),
        ]);
        let violations = check_custom_clearance(&board, &rules);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule_name.as_deref(), Some("second_large"));
        assert_eq!(violations[0].required_nm, 200_000);
    }

    #[test]
    fn custom_clearance_same_net_never_violates() {
        let (mut board, net_x, _) = two_class_board(50_000, 50_000);
        board.tracks.push(track_on(0, net_x, 0));
        board.tracks.push(track_on(1, net_x, 300_000));

        let rules = design_rules_with(vec![clearance_rule(
            "would_match_if_cross_net",
            2_000_000,
            "A.NetClass == 'HV'",
        )]);
        assert!(check_custom_clearance(&board, &rules).is_empty());
    }
}
