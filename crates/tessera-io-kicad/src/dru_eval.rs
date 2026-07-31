//! Evaluates a `.kicad_dru` condition [`crate::dru_expr::Expr`] against a
//! board item's facts, and resolves which constraint applies to a given
//! item when multiple rules match.
//!
//! Scoped deliberately: this module is a pure function of [`ItemFacts`],
//! not of a real `tessera_model::Board` — populating `ItemFacts` for a
//! real item (tracing a net's full connectivity to find its two terminal
//! pads for `fromTo`, precomputing which named rule areas an item's
//! geometry actually intersects) is separate follow-up work, not
//! attempted here. Keeping this module pure makes every semantic rule
//! below independently testable against synthetic facts, the same way
//! `dru_expr`'s parser is tested against raw strings without needing a
//! real board.
//!
//! Every rule implemented here is empirically grounded against real
//! `kicad-cli` — see `docs/DECISIONS.md`'s "Wildcard/pairwise-binding
//! semantics" entry:
//!
//! - `fromTo`/pairwise `NetClass` conditions are **symmetric**: `A`/`B`
//!   bind to the two compared facts in either order, so this module tries
//!   both and matches if either succeeds.
//! - `fromTo` matches a pattern against **two candidate strings** per
//!   endpoint: `"reference-number"` (e.g. `"IC14-3"`) and bare
//!   `"reference"` alone — case-**insensitively**.
//! - `intersectsArea`/`insideArea` match against a rule area's name,
//!   case-**sensitively**. (Both predicates are treated identically here,
//!   matching the empirical finding that they behave the same for real
//!   board items in KiCad 10.0.3 — see the "ADR-0002 addendum" entries.)
//! - `inDiffPair` matches against a diff pair's *base name* (net name with
//!   a `_P`/`_N` or `+`/`-` suffix stripped), case-**sensitively**. A net
//!   is part of a diff pair only if a companion net exists with the same
//!   base name and the opposite suffix — this needs no persistent pairing
//!   model, only the two net names being compared at evaluation time.
//! - When multiple rules with the *same constraint kind* match the same
//!   item, only the **last-declared** rule (in file order) applies —
//!   earlier matches are superseded, not combined.

use crate::dru::{Constraint, Rule};
use crate::dru_expr::{Expr, Predicate};

/// The facts a `.kicad_dru` condition is evaluated against for one board
/// item. Every field is optional/empty by default so a caller that only
/// knows some facts (e.g. no diff-pair partner exists on the board) can
/// still evaluate whatever predicates it can answer.
#[derive(Debug, Clone, Default)]
pub struct ItemFacts<'a> {
    pub net_class: Option<&'a str>,
    /// This item's own net name, e.g. `"SIG_P"` — used to derive
    /// diff-pair base-name matching for `inDiffPair`.
    pub net_name: Option<&'a str>,
    /// This net's diff-pair companion's name, if one exists on the board
    /// (e.g. `"SIG_N"` when this item's net is `"SIG_P"`) — `None` if no
    /// companion net exists, regardless of whether `net_name` itself
    /// looks suffix-shaped.
    pub diff_pair_partner_net_name: Option<&'a str>,
    /// Names of every rule area this item's own geometry intersects.
    pub areas: &'a [&'a str],
    /// The two terminal pads of this item's net/connection, as
    /// `(reference, number)` pairs — e.g. `(Some("IC14"), Some("3"))`.
    /// `None` for an item with no owning connection endpoints (e.g. a
    /// board with no 2-pin connectivity modelled yet), or when either
    /// endpoint pad's reference/number is itself unknown.
    pub connection_endpoints: Option<[EndpointFacts<'a>; 2]>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EndpointFacts<'a> {
    pub reference: Option<&'a str>,
    pub number: Option<&'a str>,
}

impl EndpointFacts<'_> {
    fn candidates(self) -> Vec<String> {
        let Some(reference) = self.reference else {
            return Vec::new();
        };
        let mut candidates = vec![reference.to_string()];
        if let Some(number) = self.number {
            candidates.push(format!("{reference}-{number}"));
        }
        candidates
    }
}

/// Evaluates `expr` against `facts`. A predicate whose required fact is
/// missing from `facts` (e.g. `fromTo` with no known connection
/// endpoints) evaluates to `false` rather than erroring — an unknown fact
/// simply can't satisfy a condition about it, the same way a missing net
/// class already means "no clearance rule applies" elsewhere in this
/// workspace (`tessera_model::Board::resolved_clearance_nm`'s docs).
#[must_use]
pub fn eval(expr: &Expr, facts: &ItemFacts) -> bool {
    match expr {
        Expr::Predicate { predicate, .. } => eval_predicate(predicate, facts),
        Expr::Not(inner) => !eval(inner, facts),
        Expr::And(lhs, rhs) => eval(lhs, facts) && eval(rhs, facts),
        Expr::Or(lhs, rhs) => eval(lhs, facts) || eval(rhs, facts),
    }
}

fn eval_predicate(predicate: &Predicate, facts: &ItemFacts) -> bool {
    match predicate {
        Predicate::NetClassEq(pattern) => facts.net_class == Some(pattern.as_str()),
        Predicate::NetClassNeq(pattern) => facts.net_class != Some(pattern.as_str()),
        Predicate::InDiffPair(pattern) => match diff_pair_base_name(facts) {
            Some(base) => glob_match(pattern, base, Case::Sensitive),
            None => false,
        },
        Predicate::IntersectsArea(pattern) | Predicate::InsideArea(pattern) => facts
            .areas
            .iter()
            .any(|area| glob_match(pattern, area, Case::Sensitive)),
        Predicate::FromTo(a, b) => match &facts.connection_endpoints {
            Some([e0, e1]) => from_to_matches(a, b, *e0, *e1) || from_to_matches(a, b, *e1, *e0),
            None => false,
        },
    }
}

/// A net is in a diff pair only if a companion net with the opposite
/// suffix actually exists (`facts.diff_pair_partner_net_name.is_some()`),
/// not merely because its own name happens to look suffix-shaped — see
/// this module's doc comment.
fn diff_pair_base_name<'a>(facts: &ItemFacts<'a>) -> Option<&'a str> {
    let name = facts.net_name?;
    facts.diff_pair_partner_net_name?;
    strip_diff_pair_suffix(name)
}

const DIFF_PAIR_SUFFIXES: [&str; 4] = ["_P", "_N", "+", "-"];

fn strip_diff_pair_suffix(name: &str) -> Option<&str> {
    DIFF_PAIR_SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
}

fn from_to_matches(a: &str, b: &str, from: EndpointFacts, to: EndpointFacts) -> bool {
    let from_candidates = from.candidates();
    let to_candidates = to.candidates();
    let matches_any = |pattern: &str, candidates: &[String]| {
        candidates
            .iter()
            .any(|c| glob_match(pattern, c, Case::Insensitive))
    };
    matches_any(a, &from_candidates) && matches_any(b, &to_candidates)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Case {
    Sensitive,
    Insensitive,
}

/// Matches `pattern` (which may contain `*` wildcards, each matching any
/// run of characters including none) against `text`. Handles any number
/// of `*`s, not just a single leading/trailing one — none of the
/// `.kicad_dru` examples this crate has seen use more than one, but nothing
/// about the empirical grounding rules that out, so this doesn't either.
fn glob_match(pattern: &str, text: &str, case: Case) -> bool {
    let (pattern, text): (String, String) = match case {
        Case::Sensitive => (pattern.to_string(), text.to_string()),
        Case::Insensitive => (pattern.to_lowercase(), text.to_lowercase()),
    };

    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }

    let first = parts[0];
    let last = parts[parts.len() - 1];
    let Some(mut remaining) = text.strip_prefix(first) else {
        return false;
    };

    for middle in &parts[1..parts.len() - 1] {
        if middle.is_empty() {
            continue;
        }
        let Some(found_at) = remaining.find(middle) else {
            return false;
        };
        remaining = &remaining[found_at + middle.len()..];
    }

    remaining.len() >= last.len() && remaining.ends_with(last)
}

/// Selects, for `kind` (a constraint kind like `"track_width"`), the
/// *last*-declared rule in `rules` whose condition matches `facts` and
/// which has a constraint of that kind — the empirically-verified
/// last-wins resolution this module's doc comment describes. Returns the
/// matching `(rule, constraint)` pair (the rule, so a caller can report
/// which rule's name a violation came from, matching how real `kicad-cli`
/// violation descriptions cite the rule by name) — `None` if no rule with
/// that constraint kind matches at all.
#[must_use]
pub fn resolve_constraint<'a>(
    rules: &'a [Rule],
    facts: &ItemFacts,
    kind: &str,
) -> Option<(&'a Rule, &'a Constraint)> {
    rules
        .iter()
        .filter(|rule| {
            rule.condition
                .as_deref()
                .is_none_or(|condition_text| condition_matches(condition_text, facts))
        })
        .flat_map(|rule| {
            rule.constraints
                .iter()
                .filter(|c| c.kind == kind)
                .map(move |c| (rule, c))
        })
        .next_back()
}

/// A malformed/unparseable condition string is treated as non-matching
/// (fail closed, not open) rather than making the whole rule apply
/// unconditionally — a rule whose condition this parser can't understand
/// yet should never silently become unconditional.
fn condition_matches(condition_text: &str, facts: &ItemFacts) -> bool {
    crate::dru_expr::parse_condition(condition_text).is_ok_and(|expr| eval(&expr, facts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn predicate(subject: &str, predicate: Predicate) -> Expr {
        Expr::Predicate {
            subject: subject.to_string(),
            predicate,
        }
    }

    #[test]
    fn glob_match_handles_prefix_suffix_and_exact() {
        assert!(glob_match("IC14-*", "IC14-3", Case::Sensitive));
        assert!(glob_match("*-3", "IC14-3", Case::Sensitive));
        assert!(glob_match("IC14", "IC14", Case::Sensitive));
        assert!(!glob_match("IC14", "IC14-3", Case::Sensitive));
        assert!(!glob_match("IC1", "IC14-3", Case::Sensitive));
    }

    #[test]
    fn glob_match_handles_multiple_wildcards() {
        assert!(glob_match("A*B*C", "AxxBxxC", Case::Sensitive));
        assert!(glob_match("A*B*C", "ABC", Case::Sensitive));
        assert!(!glob_match("A*B*C", "ACB", Case::Sensitive));
    }

    #[test]
    fn glob_match_case_sensitivity() {
        assert!(glob_match("shield*", "shieldzone", Case::Sensitive));
        assert!(!glob_match("shield*", "ShieldZone", Case::Sensitive));
        assert!(glob_match("shield*", "ShieldZone", Case::Insensitive));
    }

    #[test]
    fn net_class_eq_and_neq() {
        let facts = ItemFacts {
            net_class: Some("Power"),
            ..Default::default()
        };
        assert!(eval_predicate(
            &Predicate::NetClassEq("Power".to_string()),
            &facts
        ));
        assert!(!eval_predicate(
            &Predicate::NetClassEq("Signal".to_string()),
            &facts
        ));
        assert!(eval_predicate(
            &Predicate::NetClassNeq("Signal".to_string()),
            &facts
        ));
        assert!(!eval_predicate(
            &Predicate::NetClassNeq("Power".to_string()),
            &facts
        ));
    }

    #[test]
    fn in_diff_pair_requires_a_real_partner_net_not_just_suffix_shaped_name() {
        let with_partner = ItemFacts {
            net_name: Some("SIG_P"),
            diff_pair_partner_net_name: Some("SIG_N"),
            ..Default::default()
        };
        assert!(eval_predicate(
            &Predicate::InDiffPair("SIG".to_string()),
            &with_partner
        ));
        assert!(eval_predicate(
            &Predicate::InDiffPair("*".to_string()),
            &with_partner
        ));
        assert!(!eval_predicate(
            &Predicate::InDiffPair("OTHER".to_string()),
            &with_partner
        ));

        let no_partner = ItemFacts {
            net_name: Some("SIG_P"),
            diff_pair_partner_net_name: None,
            ..Default::default()
        };
        assert!(!eval_predicate(
            &Predicate::InDiffPair("*".to_string()),
            &no_partner
        ));
    }

    #[test]
    fn in_diff_pair_base_name_is_case_sensitive() {
        let facts = ItemFacts {
            net_name: Some("SIG_P"),
            diff_pair_partner_net_name: Some("SIG_N"),
            ..Default::default()
        };
        assert!(!eval_predicate(
            &Predicate::InDiffPair("sig".to_string()),
            &facts
        ));
    }

    #[test]
    fn intersects_area_and_inside_area_both_match_the_same_area_list() {
        let facts = ItemFacts {
            areas: &["ShieldZoneA"],
            ..Default::default()
        };
        assert!(eval_predicate(
            &Predicate::IntersectsArea("Shield*".to_string()),
            &facts
        ));
        assert!(eval_predicate(
            &Predicate::InsideArea("Shield*".to_string()),
            &facts
        ));
        assert!(!eval_predicate(
            &Predicate::IntersectsArea("shield*".to_string()),
            &facts
        ));
    }

    #[test]
    fn from_to_matches_symmetrically_and_case_insensitively() {
        let facts = ItemFacts {
            connection_endpoints: Some([
                EndpointFacts {
                    reference: Some("IC14"),
                    number: Some("3"),
                },
                EndpointFacts {
                    reference: Some("IC13"),
                    number: Some("7"),
                },
            ]),
            ..Default::default()
        };
        assert!(eval_predicate(
            &Predicate::FromTo("ic14-*".to_string(), "IC13-*".to_string()),
            &facts
        ));
        // reversed order should also match (symmetric, per empirical
        // finding 1 in docs/DECISIONS.md).
        assert!(eval_predicate(
            &Predicate::FromTo("IC13-*".to_string(), "IC14-*".to_string()),
            &facts
        ));
        // bare reference, no pad-number suffix, also matches.
        assert!(eval_predicate(
            &Predicate::FromTo("IC14".to_string(), "IC13".to_string()),
            &facts
        ));
        // wrong pad number does not match.
        assert!(!eval_predicate(
            &Predicate::FromTo("IC14-99".to_string(), "IC13-*".to_string()),
            &facts
        ));
    }

    #[test]
    fn from_to_with_no_known_endpoints_never_matches() {
        let facts = ItemFacts::default();
        assert!(!eval_predicate(
            &Predicate::FromTo("*".to_string(), "*".to_string()),
            &facts
        ));
    }

    #[test]
    fn eval_handles_not_and_and_or() {
        let facts = ItemFacts {
            net_class: Some("HV"),
            areas: &[],
            ..Default::default()
        };
        let expr = Expr::And(
            Box::new(predicate("A", Predicate::NetClassEq("HV".to_string()))),
            Box::new(Expr::Not(Box::new(predicate(
                "A",
                Predicate::InsideArea("Shield*".to_string()),
            )))),
        );
        assert!(eval(&expr, &facts));

        let shielded = ItemFacts {
            net_class: Some("HV"),
            areas: &["ShieldZone"],
            ..Default::default()
        };
        assert!(!eval(&expr, &shielded));
    }

    #[test]
    fn resolve_constraint_picks_the_last_matching_rule() {
        let rules = vec![
            Rule {
                name: "first".to_string(),
                layer: Vec::new(),
                constraints: vec![Constraint {
                    kind: "track_width".to_string(),
                    min_nm: Some(100_000),
                    max_nm: None,
                    opt_nm: None,
                    args: Vec::new(),
                }],
                condition: Some("A.NetClass == 'X'".to_string()),
            },
            Rule {
                name: "second".to_string(),
                layer: Vec::new(),
                constraints: vec![Constraint {
                    kind: "track_width".to_string(),
                    min_nm: Some(200_000),
                    max_nm: None,
                    opt_nm: None,
                    args: Vec::new(),
                }],
                condition: Some("A.NetClass == 'X'".to_string()),
            },
        ];
        let facts = ItemFacts {
            net_class: Some("X"),
            ..Default::default()
        };
        let (rule, constraint) = resolve_constraint(&rules, &facts, "track_width").unwrap();
        assert_eq!(rule.name, "second", "second (last) rule should win");
        assert_eq!(constraint.min_nm, Some(200_000));
    }

    #[test]
    fn resolve_constraint_skips_rules_whose_condition_does_not_match() {
        let rules = vec![Rule {
            name: "only".to_string(),
            layer: Vec::new(),
            constraints: vec![Constraint {
                kind: "track_width".to_string(),
                min_nm: Some(100_000),
                max_nm: None,
                opt_nm: None,
                args: Vec::new(),
            }],
            condition: Some("A.NetClass == 'Y'".to_string()),
        }];
        let facts = ItemFacts {
            net_class: Some("X"),
            ..Default::default()
        };
        assert!(resolve_constraint(&rules, &facts, "track_width").is_none());
    }

    #[test]
    fn resolve_constraint_treats_a_ruleless_condition_as_unconditional() {
        let rules = vec![Rule {
            name: "always".to_string(),
            layer: Vec::new(),
            constraints: vec![Constraint {
                kind: "clearance".to_string(),
                min_nm: Some(100_000),
                max_nm: None,
                opt_nm: None,
                args: Vec::new(),
            }],
            condition: None,
        }];
        let facts = ItemFacts::default();
        assert!(resolve_constraint(&rules, &facts, "clearance").is_some());
    }
}
