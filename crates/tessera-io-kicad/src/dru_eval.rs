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
//! - `fromTo(a, b)` matches if *any two distinct* known endpoints satisfy
//!   `a`/`b` respectively, not just physically-adjacent ones — verified
//!   against a multi-pin net (see `from_to_matches_any_pair`'s docs and
//!   `docs/DECISIONS.md`'s "Multi-pin `fromTo`/`length` support" entry).
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
    /// Every pad `fromTo` may match against for this item — e.g. a 2-pin
    /// connection's two terminal pads, or (for a whole-net check like
    /// `length`) every pad on the net. Empty for an item with no known
    /// connection endpoints at all. `fromTo(a, b)` matches if *some two
    /// distinct* endpoints in this list satisfy `a`/`b` respectively (in
    /// either order) — see [`from_to_matches_any_pair`] for why "distinct"
    /// matters and how this generalizes the exactly-2-endpoint case.
    pub connection_endpoints: Vec<EndpointFacts<'a>>,
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
///
/// Every constraint kind implemented so far (`track_width`, `disallow`,
/// `length`) is single-subject — there's only one real item in play, so
/// `A`/`B` are indistinguishable and this function doesn't need to care
/// which one a predicate names. `eval` is exactly [`eval_pairwise`] called
/// with the same facts on both sides — see that function's docs for the
/// genuinely two-item case (`clearance`).
#[must_use]
pub fn eval(expr: &Expr, facts: &ItemFacts) -> bool {
    eval_with_binding(expr, facts, facts)
}

/// Evaluates a genuinely pairwise condition — currently only `clearance`,
/// the sole constraint kind where `A` and `B` name two *different* real
/// items being compared, not the same item under two names. Tries both
/// assignments of which item binds to `A` vs `B` and matches if *either*
/// satisfies the whole expression.
///
/// Empirically verified against real `kicad-cli` (see `docs/DECISIONS.md`'s
/// clearance-pairwise-semantics entry), not assumed by generalising the
/// single-subject finding:
/// - `A.NetClass=='HV' && B.NetClass=='LV'` matches a pair regardless of
///   which physical item the rule happens to name first — the reversed
///   condition `A.NetClass=='LV' && B.NetClass=='HV'` matches the *same*
///   pair.
/// - A condition naming only `A` (no `B` at all) matches if *either* item
///   in the pair satisfies it, not only a specific one.
#[must_use]
pub fn eval_pairwise(expr: &Expr, facts_x: &ItemFacts, facts_y: &ItemFacts) -> bool {
    eval_with_binding(expr, facts_x, facts_y) || eval_with_binding(expr, facts_y, facts_x)
}

fn eval_with_binding(expr: &Expr, facts_a: &ItemFacts, facts_b: &ItemFacts) -> bool {
    match expr {
        Expr::Predicate { subject, predicate } => {
            let facts = if subject == "B" { facts_b } else { facts_a };
            eval_predicate(predicate, facts)
        }
        Expr::Not(inner) => !eval_with_binding(inner, facts_a, facts_b),
        Expr::And(lhs, rhs) => {
            eval_with_binding(lhs, facts_a, facts_b) && eval_with_binding(rhs, facts_a, facts_b)
        }
        Expr::Or(lhs, rhs) => {
            eval_with_binding(lhs, facts_a, facts_b) || eval_with_binding(rhs, facts_a, facts_b)
        }
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
        Predicate::FromTo(a, b) => from_to_matches_any_pair(a, b, &facts.connection_endpoints),
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

/// `fromTo(a, b)` matches `endpoints` if some two *distinct* endpoints
/// satisfy `a`/`b` respectively — trying every ordered pair, not just
/// adjacent ones, so this works the same for a 2-pin connection's two
/// terminals and a multi-pin net's full pad list.
///
/// **Verified against real `kicad-cli` on a 3-pad daisy-chained net**
/// (`J1`—track—`J2`—track—`J3`, all one net): `A.fromTo('J1','J3')` and
/// `A.fromTo('J2','J3')` both match a `length` rule checking that net's
/// *whole* routed length — i.e. `fromTo` isn't limited to physically
/// adjacent pad pairs, any two distinct pads on the connection satisfy
/// it. Requiring **distinct** endpoints (not just "some endpoint matches
/// `a`, some endpoint matches `b`, possibly the same one") matters
/// because it's the only generalization that reduces *exactly* to the
/// original 2-endpoint behavior — see `docs/DECISIONS.md`'s "Multi-pin
/// `fromTo`/`length` support" entry for the reasoning and the fixtures
/// that pinned this down.
fn from_to_matches_any_pair(a: &str, b: &str, endpoints: &[EndpointFacts]) -> bool {
    endpoints.iter().enumerate().any(|(i, from)| {
        endpoints
            .iter()
            .enumerate()
            .any(|(j, to)| i != j && from_to_matches(a, b, *from, *to))
    })
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
        .filter(|rule| rule_condition_matches(rule, facts))
        .flat_map(|rule| {
            rule.constraints
                .iter()
                .filter(|c| c.kind == kind)
                .map(move |c| (rule, c))
        })
        .next_back()
}

/// Resolves whether `item_type` (a bare token like `"track"`/`"via"`,
/// matching a `disallow` constraint's own vocabulary — see
/// `crate::dru::Constraint::args`' docs) is disallowed for the item
/// described by `facts`.
///
/// **Not simply `resolve_constraint(rules, facts, "disallow")` plus an
/// `args` check on the result** — verified against real `kicad-cli` that
/// last-wins competition for `disallow` is scoped to rules whose `args`
/// actually include `item_type`, not to every `disallow`-kind rule
/// regardless of which item types it lists. Concretely: a rule disallowing
/// only `via` does **not** suppress an earlier rule disallowing `track`
/// for a track item, even though both are `disallow`-kind rules with the
/// same matching condition — `resolve_constraint`'s generic
/// kind-only scoping would incorrectly let the later `via`-only rule
/// "win" and silently clear the track's real violation.
#[must_use]
pub fn resolve_disallow<'a>(
    rules: &'a [Rule],
    facts: &ItemFacts,
    item_type: &str,
) -> Option<&'a Rule> {
    rules.iter().rfind(|rule| {
        rule_condition_matches(rule, facts)
            && rule
                .constraints
                .iter()
                .any(|c| c.kind == "disallow" && c.args.iter().any(|a| a == item_type))
    })
}

/// Pairwise analog of [`resolve_constraint`] for genuinely two-item
/// constraint kinds (currently only `clearance`): selects the *last*-
/// declared rule whose condition matches the pair via [`eval_pairwise`].
///
/// Uses the same simple kind-only-scoped last-wins as `resolve_constraint`
/// (`track_width` and now `clearance`), **not** [`resolve_disallow`]'s
/// item-type-scoped variant — verified directly, not assumed just because
/// `clearance` is bound-shaped like `track_width`: two competing
/// `clearance` rules with the same matching condition and different
/// bounds, checked against real `kicad-cli`, resolve to the *last*
/// declared one, the same simple behaviour `track_width` already has (see
/// `docs/DECISIONS.md`'s clearance-pairwise-semantics entry) — `clearance`
/// has no `args` item-type list to scope by in the first place, unlike
/// `disallow`.
#[must_use]
pub fn resolve_constraint_pairwise<'a>(
    rules: &'a [Rule],
    facts_x: &ItemFacts,
    facts_y: &ItemFacts,
    kind: &str,
) -> Option<(&'a Rule, &'a Constraint)> {
    rules
        .iter()
        .filter(|rule| rule_condition_matches_pairwise(rule, facts_x, facts_y))
        .flat_map(|rule| {
            rule.constraints
                .iter()
                .filter(|c| c.kind == kind)
                .map(move |c| (rule, c))
        })
        .next_back()
}

fn rule_condition_matches(rule: &Rule, facts: &ItemFacts) -> bool {
    rule.condition
        .as_deref()
        .is_none_or(|condition_text| condition_matches(condition_text, facts))
}

fn rule_condition_matches_pairwise(rule: &Rule, facts_x: &ItemFacts, facts_y: &ItemFacts) -> bool {
    rule.condition
        .as_deref()
        .is_none_or(|condition_text| condition_matches_pairwise(condition_text, facts_x, facts_y))
}

/// A malformed/unparseable condition string is treated as non-matching
/// (fail closed, not open) rather than making the whole rule apply
/// unconditionally — a rule whose condition this parser can't understand
/// yet should never silently become unconditional.
fn condition_matches(condition_text: &str, facts: &ItemFacts) -> bool {
    crate::dru_expr::parse_condition(condition_text).is_ok_and(|expr| eval(&expr, facts))
}

fn condition_matches_pairwise(
    condition_text: &str,
    facts_x: &ItemFacts,
    facts_y: &ItemFacts,
) -> bool {
    crate::dru_expr::parse_condition(condition_text)
        .is_ok_and(|expr| eval_pairwise(&expr, facts_x, facts_y))
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
            connection_endpoints: vec![
                EndpointFacts {
                    reference: Some("IC14"),
                    number: Some("3"),
                },
                EndpointFacts {
                    reference: Some("IC13"),
                    number: Some("7"),
                },
            ],
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
    fn from_to_matches_any_two_distinct_endpoints_on_a_multi_pin_net() {
        // Verified against real kicad-cli on a 3-pad daisy-chained net
        // (docs/DECISIONS.md's "Multi-pin fromTo/length support" entry):
        // fromTo matches any two distinct named pads on the connection,
        // not just physically-adjacent ones.
        let facts = ItemFacts {
            connection_endpoints: vec![
                EndpointFacts {
                    reference: Some("J1"),
                    number: Some("1"),
                },
                EndpointFacts {
                    reference: Some("J2"),
                    number: Some("1"),
                },
                EndpointFacts {
                    reference: Some("J3"),
                    number: Some("1"),
                },
            ],
            ..Default::default()
        };
        assert!(eval_predicate(
            &Predicate::FromTo("J1".to_string(), "J3".to_string()),
            &facts
        ));
        assert!(eval_predicate(
            &Predicate::FromTo("J2".to_string(), "J3".to_string()),
            &facts
        ));
        assert!(eval_predicate(
            &Predicate::FromTo("J1".to_string(), "J2".to_string()),
            &facts
        ));
        assert!(!eval_predicate(
            &Predicate::FromTo("J1".to_string(), "J4".to_string()),
            &facts
        ));
    }

    #[test]
    fn from_to_requires_two_distinct_endpoints_not_the_same_one_twice() {
        // A pattern that could match the *same* single endpoint on both
        // sides must not self-satisfy — fromTo means "between two
        // different pads," not "this pad exists." This is what makes
        // `from_to_matches_any_pair`'s distinct-index requirement the
        // correct generalization of the original fixed-2-endpoint
        // behavior, not just a looser "any/any" check.
        let facts = ItemFacts {
            connection_endpoints: vec![
                EndpointFacts {
                    reference: Some("IC14"),
                    number: Some("3"),
                },
                EndpointFacts {
                    reference: Some("IC13"),
                    number: Some("7"),
                },
            ],
            ..Default::default()
        };
        assert!(!eval_predicate(
            &Predicate::FromTo("IC14".to_string(), "IC14".to_string()),
            &facts
        ));
        // a wildcard on both sides still needs two distinct endpoints to
        // pair against, but this board has exactly two, so it matches.
        assert!(eval_predicate(
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

    fn disallow_rule(name: &str, item_types: &[&str], condition: &str) -> Rule {
        Rule {
            name: name.to_string(),
            layer: Vec::new(),
            constraints: vec![Constraint {
                kind: "disallow".to_string(),
                min_nm: None,
                max_nm: None,
                opt_nm: None,
                args: item_types.iter().map(ToString::to_string).collect(),
            }],
            condition: Some(condition.to_string()),
        }
    }

    #[test]
    fn resolve_disallow_is_not_suppressed_by_a_later_rule_for_a_different_item_type() {
        // Verified against real kicad-cli: a later `disallow via` rule
        // must not suppress an earlier `disallow track` rule for a track
        // item, even though both are disallow-kind rules with an
        // identically-matching condition.
        let rules = vec![
            disallow_rule("disallow_track", &["track"], "A.NetClass == 'Default'"),
            disallow_rule("disallow_via_only", &["via"], "A.NetClass == 'Default'"),
        ];
        let facts = ItemFacts {
            net_class: Some("Default"),
            ..Default::default()
        };
        let resolved = resolve_disallow(&rules, &facts, "track").unwrap();
        assert_eq!(resolved.name, "disallow_track");
    }

    #[test]
    fn resolve_disallow_last_wins_when_rules_genuinely_compete() {
        // Verified against real kicad-cli: when two rules both disallow
        // the *same* item type and both match, only the last-declared one
        // is reported.
        let rules = vec![
            disallow_rule("disallow_first", &["track"], "A.NetClass == 'Default'"),
            disallow_rule("disallow_second", &["track"], "A.NetClass == 'Default'"),
        ];
        let facts = ItemFacts {
            net_class: Some("Default"),
            ..Default::default()
        };
        let resolved = resolve_disallow(&rules, &facts, "track").unwrap();
        assert_eq!(resolved.name, "disallow_second");
    }

    #[test]
    fn resolve_disallow_is_none_when_no_rule_lists_this_item_type() {
        let rules = vec![disallow_rule(
            "disallow_via_only",
            &["via"],
            "A.NetClass == 'Default'",
        )];
        let facts = ItemFacts {
            net_class: Some("Default"),
            ..Default::default()
        };
        assert!(resolve_disallow(&rules, &facts, "track").is_none());
    }

    #[test]
    fn resolve_disallow_is_none_when_condition_does_not_match() {
        let rules = vec![disallow_rule(
            "disallow_track",
            &["track"],
            "A.NetClass == 'Other'",
        )];
        let facts = ItemFacts {
            net_class: Some("Default"),
            ..Default::default()
        };
        assert!(resolve_disallow(&rules, &facts, "track").is_none());
    }

    // Verified against real kicad-cli (docs/DECISIONS.md's
    // clearance-pairwise-semantics entry): A/B in a pairwise condition
    // bind to whichever real item satisfies them, tried in both
    // assignments, not to a fixed "first item is always A" order.

    fn net_class_facts(class: &str) -> ItemFacts<'_> {
        ItemFacts {
            net_class: Some(class),
            ..Default::default()
        }
    }

    #[test]
    fn eval_pairwise_matches_regardless_of_which_item_the_rule_names_first() {
        let hv = net_class_facts("HV");
        let lv = net_class_facts("LV");
        let forward = Expr::And(
            Box::new(predicate("A", Predicate::NetClassEq("HV".to_string()))),
            Box::new(predicate("B", Predicate::NetClassEq("LV".to_string()))),
        );
        let reversed = Expr::And(
            Box::new(predicate("A", Predicate::NetClassEq("LV".to_string()))),
            Box::new(predicate("B", Predicate::NetClassEq("HV".to_string()))),
        );
        assert!(eval_pairwise(&forward, &hv, &lv));
        assert!(
            eval_pairwise(&reversed, &hv, &lv),
            "reversed condition must match the same physical pair"
        );
    }

    #[test]
    fn eval_pairwise_single_subject_condition_matches_if_either_item_qualifies() {
        let hv = net_class_facts("HV");
        let other = net_class_facts("Default");
        let a_only = predicate("A", Predicate::NetClassEq("HV".to_string()));
        assert!(eval_pairwise(&a_only, &hv, &other));
        assert!(
            eval_pairwise(&a_only, &other, &hv),
            "order of the two items passed in must not matter"
        );
    }

    #[test]
    fn eval_pairwise_does_not_match_when_neither_item_qualifies() {
        let a = net_class_facts("Default");
        let b = net_class_facts("Other");
        let expr = predicate("A", Predicate::NetClassEq("HV".to_string()));
        assert!(!eval_pairwise(&expr, &a, &b));
    }

    #[test]
    fn eval_reduces_to_eval_pairwise_with_the_same_facts_on_both_sides() {
        let facts = net_class_facts("HV");
        let expr = predicate("B", Predicate::NetClassEq("HV".to_string()));
        assert_eq!(eval(&expr, &facts), eval_pairwise(&expr, &facts, &facts));
        assert!(eval(&expr, &facts));
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

    #[test]
    fn resolve_constraint_pairwise_matches_regardless_of_item_order() {
        let hv = net_class_facts("HV");
        let lv = net_class_facts("LV");
        let rules = vec![clearance_rule(
            "hv_lv",
            2_000_000,
            "A.NetClass == 'HV' && B.NetClass == 'LV'",
        )];
        let (rule, constraint) =
            resolve_constraint_pairwise(&rules, &hv, &lv, "clearance").unwrap();
        assert_eq!(rule.name, "hv_lv");
        assert_eq!(constraint.min_nm, Some(2_000_000));
        // Same pair, arguments swapped: still resolves.
        assert!(resolve_constraint_pairwise(&rules, &lv, &hv, "clearance").is_some());
    }

    #[test]
    fn resolve_constraint_pairwise_last_wins_like_resolve_constraint() {
        // Verified against real kicad-cli: two competing clearance rules
        // with the same matching condition resolve to the last-declared
        // one — the same simple kind-only-scoped last-wins
        // resolve_constraint already has, not resolve_disallow's
        // item-type-scoped variant (clearance has no args list at all).
        let a = net_class_facts("Default");
        let b = net_class_facts("Default");
        let rules = vec![
            clearance_rule("first_small", 50_000, "A.NetClass == 'Default'"),
            clearance_rule("second_large", 200_000, "A.NetClass == 'Default'"),
        ];
        let (rule, constraint) = resolve_constraint_pairwise(&rules, &a, &b, "clearance").unwrap();
        assert_eq!(rule.name, "second_large");
        assert_eq!(constraint.min_nm, Some(200_000));
    }

    #[test]
    fn resolve_constraint_pairwise_is_none_when_no_rule_matches() {
        let a = net_class_facts("Default");
        let b = net_class_facts("Default");
        let rules = vec![clearance_rule("only", 50_000, "A.NetClass == 'Other'")];
        assert!(resolve_constraint_pairwise(&rules, &a, &b, "clearance").is_none());
    }
}
