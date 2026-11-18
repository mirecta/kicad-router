//! `.kicad_dru` custom rules resolved for routing: the router itself
//! doesn't speak the rule language, so the rules that depend on nets alone
//! are evaluated here, once, into a [`RuleOverrides`] table the model's own
//! resolvers (`Board::resolved_clearance_nm`, `track_width_for`,
//! `via_size_for`) consult — so the grid, rip-up and the router's own
//! checks all see the same numbers.
//!
//! Conservative by construction: every value is the stricter of the rule
//! and the net classes (KiCad lets a matching rule replace the class value
//! outright, so routing to the stricter one is always legal), and a rule
//! whose condition needs geometry (`insideArea`, `intersectsArea`,
//! `fromTo`) or can't be parsed is left out and reported — never applied
//! everywhere by accident.

use tessera_model::{Board, NetId, RuleOverrides};

use crate::dru::{Constraint, DesignRules, Rule};
use crate::dru_eval::{self, ItemFacts};
use crate::dru_expr::{self, Expr, Predicate};

/// Resolves `rules` for every net (and pair of nets) on `board`. Returns
/// the table and a note per rule the router can't apply.
#[must_use]
pub fn routing_overrides(board: &Board, rules: &DesignRules) -> (RuleOverrides, Vec<String>) {
    let (usable, notes) = usable_rules(rules);
    let mut overrides = RuleOverrides::default();
    if usable.is_empty() {
        return (overrides, notes);
    }
    let mut nets: Vec<NetId> = board.nets.keys().copied().collect();
    nets.sort();
    let partners: Vec<Option<String>> = nets
        .iter()
        .map(|&n| {
            board
                .diff_pair_partner(n)
                .and_then(|p| board.nets.get(&p))
                .map(|p| p.name.clone())
        })
        .collect();
    let facts: Vec<ItemFacts> = nets
        .iter()
        .zip(&partners)
        .map(|(n, partner)| {
            let net = &board.nets[n];
            ItemFacts {
                net_class: Some(net.net_class.as_str()),
                net_name: Some(net.name.as_str()),
                diff_pair_partner_net_name: partner.as_deref(),
                ..ItemFacts::default()
            }
        })
        .collect();

    for (i, &a) in nets.iter().enumerate() {
        for (j, &b) in nets.iter().enumerate().skip(i + 1) {
            let Some(class_nm) = board.resolved_clearance_nm(a, b) else {
                continue;
            };
            if let Some(min_nm) =
                dru_eval::resolve_constraint_pairwise(&usable, &facts[i], &facts[j], "clearance")
                    .and_then(|(_, c)| c.min_nm)
            {
                if min_nm > class_nm {
                    overrides.clearance_nm.insert((a, b), min_nm);
                }
            }
        }
    }
    for (i, &net) in nets.iter().enumerate() {
        net_sizes(board, &usable, &facts[i], net, &mut overrides);
    }
    (overrides, notes)
}

/// The rules routing can use, and a note on each it can't.
fn usable_rules(rules: &DesignRules) -> (Vec<Rule>, Vec<String>) {
    let mut notes = Vec::new();
    let usable = rules
        .rules
        .iter()
        .filter(|rule| {
            let routing_kind = rule.constraints.iter().any(|c| {
                matches!(
                    c.kind.as_str(),
                    "clearance" | "track_width" | "via_diameter" | "hole_size"
                )
            });
            if !routing_kind {
                return false;
            }
            match rule.condition.as_deref().map(dru_expr::parse_condition) {
                None => true,
                Some(Ok(expr)) if !needs_geometry(&expr) => true,
                Some(Ok(_)) => {
                    notes.push(format!(
                        "rule \"{}\" depends on areas or pins (fromTo); the router doesn't apply it \
                         — check it with DRC",
                        rule.name
                    ));
                    false
                }
                Some(Err(e)) => {
                    notes.push(format!(
                        "rule \"{}\": condition not understood ({e}); the router doesn't apply it",
                        rule.name
                    ));
                    false
                }
            }
        })
        .cloned()
        .collect();
    (usable, notes)
}

/// `value` within a constraint's bounds: its `opt` if it has one, raised
/// to its `min`, lowered to its `max`.
fn bounded(c: &Constraint, value: i64) -> i64 {
    let mut v = c.opt_nm.unwrap_or(value);
    if let Some(min) = c.min_nm {
        v = v.max(min);
    }
    if let Some(max) = c.max_nm {
        v = v.min(max);
    }
    v
}

/// One net's track width and via size under `rules`.
fn net_sizes(
    board: &Board,
    rules: &[Rule],
    facts: &ItemFacts,
    net: NetId,
    overrides: &mut RuleOverrides,
) {
    if let (Some(class_width), Some((_, c))) = (
        board.track_width_for(net),
        dru_eval::resolve_constraint(rules, facts, "track_width"),
    ) {
        let width = bounded(c, class_width);
        if width != class_width {
            overrides.track_width_nm.insert(net, width);
        }
        if let Some(min) = c.min_nm {
            overrides.min_track_width_nm.insert(net, min);
        }
    }
    let Some((class_diameter, class_drill)) = board.via_size_for(net) else {
        return;
    };
    let size = |kind: &str, class_nm: i64| {
        dru_eval::resolve_constraint(rules, facts, kind)
            .map_or(class_nm, |(_, c)| bounded(c, class_nm))
    };
    let diameter = size("via_diameter", class_diameter);
    let drill = size("hole_size", class_drill);
    if (diameter, drill) != (class_diameter, class_drill) {
        overrides.via_nm.insert(net, (diameter, drill));
    }
}

fn needs_geometry(expr: &Expr) -> bool {
    match expr {
        Expr::Predicate { predicate, .. } => matches!(
            predicate,
            Predicate::IntersectsArea(_) | Predicate::InsideArea(_) | Predicate::FromTo(..)
        ),
        Expr::Not(e) => needs_geometry(e),
        Expr::And(a, b) | Expr::Or(a, b) => needs_geometry(a) || needs_geometry(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_model::{Net, NetClass};

    fn board() -> Board {
        let mut board = Board::new();
        for (name, clearance) in [("Default", 200_000), ("HV", 200_000)] {
            let mut class = NetClass::default_placeholder();
            class.name = name.to_string();
            class.clearance_nm = clearance;
            class.track_width_nm = 250_000;
            board.net_classes.insert(name.to_string(), class);
        }
        for (id, name, class) in [
            (1, "L", "HV"),
            (2, "N", "HV"),
            (3, "SDA", "Default"),
            (4, "SCL", "Default"),
        ] {
            board.nets.insert(
                NetId(id),
                Net {
                    id: NetId(id),
                    name: name.to_string(),
                    net_class: class.to_string(),
                },
            );
        }
        board
    }

    fn rules(text: &str) -> DesignRules {
        crate::dru::parse_design_rules(text).unwrap().design_rules
    }

    #[test]
    fn a_net_class_clearance_rule_reaches_every_pair_it_matches() {
        let mut board = board();
        let (overrides, notes) = routing_overrides(
            &board,
            &rules(
                "(version 1)\n(rule \"HV\" (constraint clearance (min 3mm)) (condition \"A.NetClass == 'HV'\"))\n(rule \"wide\" (constraint track_width (min 0.5mm)) (condition \"A.NetClass == 'HV'\"))",
            ),
        );
        assert!(notes.is_empty(), "{notes:?}");
        board.overrides = overrides;
        // HV to anything, HV to HV — but not between two low-voltage nets
        assert_eq!(
            board.resolved_clearance_nm(NetId(1), NetId(3)),
            Some(3_000_000)
        );
        assert_eq!(
            board.resolved_clearance_nm(NetId(4), NetId(2)),
            Some(3_000_000)
        );
        assert_eq!(
            board.resolved_clearance_nm(NetId(1), NetId(2)),
            Some(3_000_000)
        );
        assert_eq!(
            board.resolved_clearance_nm(NetId(3), NetId(4)),
            Some(200_000)
        );
        assert_eq!(board.track_width_for(NetId(1)), Some(500_000));
        assert_eq!(board.track_width_for(NetId(3)), Some(250_000));
    }

    #[test]
    fn net_name_rules_match_with_wildcards() {
        let mut board = board();
        let (overrides, _) = routing_overrides(
            &board,
            &rules("(version 1)\n(rule \"i2c\" (constraint track_width (min 0.3mm)) (condition \"A.NetName == 'S?A' || A.NetName == 'SC*'\"))"),
        );
        board.overrides = overrides;
        assert_eq!(board.track_width_for(NetId(3)), Some(300_000));
        assert_eq!(board.track_width_for(NetId(4)), Some(300_000));
        assert_eq!(board.track_width_for(NetId(1)), Some(250_000));
    }

    #[test]
    fn a_looser_rule_never_loosens_routing_and_area_rules_are_reported() {
        let board = board();
        let (overrides, notes) = routing_overrides(
            &board,
            &rules(
                "(version 1)\n(rule \"tight\" (constraint clearance (min 0.1mm)))\n(rule \"bga\" (constraint clearance (min 1mm)) (condition \"A.insideArea('BGA')\"))",
            ),
        );
        assert!(overrides.clearance_nm.is_empty());
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("bga"), "{notes:?}");
    }
}
