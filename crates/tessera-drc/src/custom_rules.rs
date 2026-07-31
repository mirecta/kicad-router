//! Wires `tessera_io_kicad::dru_eval`'s pure evaluator to a real
//! [`Board`], checking board items against parsed `.kicad_dru`
//! [`DesignRules`].
//!
//! **Scoped to `track_width` (tracks) and `disallow` (tracks + vias) for
//! now.** Every other constraint kind needs its own measurement logic
//! this module doesn't attempt yet: `length` needs summing every track
//! segment belonging to a connection (not just one item's own geometry),
//! `clearance` is inherently pairwise (needs its own item-pair-scoped
//! `ItemFacts` construction, unlike this module's single-item-per-check
//! shape), and other bound-shaped kinds (`hole_size`, `via_diameter`,
//! `diff_pair_*`) would need their own item-type mapping. Extending this
//! module to those kinds is real follow-up work, not a gap to silently
//! paper over.
//!
//! Diff-pair partner lookup, rule-area intersection, and connection-
//! endpoint lookup (`fromTo`'s facts) are all derived fresh from `Board`
//! per item — see [`item_facts_for_net`].

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
        let areas: Vec<&str> = board
            .rule_areas
            .iter()
            .filter(|area| area.outline.contains_point(via.position))
            .map(|area| area.name.as_str())
            .collect();
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
}
