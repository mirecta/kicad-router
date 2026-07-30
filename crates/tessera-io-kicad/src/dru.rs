//! Parses `.kicad_dru` custom design-rule files (plan §7.5.6,
//! `docs/DECISIONS.md` ADR-0002) into a structured [`DesignRules`] —
//! syntax only, per ADR-0002's own phasing: "the parser (not built) before
//! any expression-evaluator work can start." A rule's `condition`
//! expression text (`A.NetClass == 'x'`, `A.inDiffPair('*')`,
//! `A.intersectsArea('name')`, `A.fromTo('ref-*','ref-*')`, boolean
//! `&&`/`||`) is captured verbatim here, not evaluated — building that
//! mini-language's evaluator, and resolving what `insideArea` vs
//! `intersectsArea` actually mean, is separate follow-up work per
//! ADR-0002, deliberately not attempted in this module.
//!
//! Grammar empirically grounded against a real shipped file
//! (`/usr/share/kicad/demos/vme-wren/vme-wren.kicad_dru`, KiCad 10.0.3;
//! its content is embedded verbatim in this module's tests), per
//! ADR-0002's Q3 finding — not guessed from the plan's assumed syntax
//! alone.

use crate::sexpr::{self, Sexpr};

#[derive(Debug, thiserror::Error)]
pub enum ParseDesignRulesError {
    #[error("failed to parse .kicad_dru syntax: {0}")]
    Syntax(#[from] sexpr::ParseError),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DesignRules {
    /// The file's declared format version — `(version N)` — `None` if
    /// absent or unparseable (KiCad always writes one in practice, but
    /// nothing here depends on it to make sense of the rules themselves).
    pub version: Option<i64>,
    pub rules: Vec<Rule>,
}

/// One `(rule "name" ...)` block. `layer` and `condition` are kept as the
/// raw text KiCad wrote — bare tokens like `outer`/`inner`/`"F.Cu"` for
/// `layer`, the mini-language expression string for `condition` — neither
/// is resolved or evaluated by this parser.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub name: String,
    /// Every atom following the `layer` tag, in order — empty if the rule
    /// has no `(layer ...)` clause at all. Only single-token layer specs
    /// (`outer`, `inner`) have actually been observed; keeping this a
    /// `Vec` rather than a single `Option<String>` means a multi-token
    /// form, if one exists, is preserved rather than silently truncated
    /// to its first token.
    pub layer: Vec<String>,
    pub constraints: Vec<Constraint>,
    pub condition: Option<String>,
}

/// One `(constraint <kind> (min ..) (max ..) (opt ..))` clause. `kind` is
/// kept as the raw token (`clearance`, `track_width`, `diff_pair_gap`,
/// `diff_pair_uncoupled`, `length`, `hole_size`, `via_diameter`, ... —
/// ADR-0002 flags this vocabulary as large and not fully enumerated, so a
/// closed enum here would mean guessing at members never actually seen).
/// Each bound is independently optional, since real rules only specify
/// whichever of min/max/opt is relevant (e.g. `diff_pair_uncoupled` only
/// ever has a `max`).
#[derive(Debug, Clone, PartialEq)]
pub struct Constraint {
    pub kind: String,
    pub min_nm: Option<i64>,
    pub max_nm: Option<i64>,
    pub opt_nm: Option<i64>,
}

pub struct ParsedDesignRules {
    pub design_rules: DesignRules,
    /// Malformed or unrecognized top-level forms, rule blocks, or
    /// constraints — skipped rather than failing the whole file, and
    /// surfaced explicitly rather than silently dropped, matching
    /// `crate::parser::ParsedBoard::warnings`'s no-silent-data-loss stance.
    pub warnings: Vec<String>,
}

/// Parses `dru_text` (a `.kicad_dru` file's contents) into
/// [`ParsedDesignRules`].
///
/// # Errors
///
/// Returns an error only if `dru_text` isn't valid S-expression syntax at
/// all (unterminated list/string). Anything semantically malformed within
/// otherwise-valid syntax (a `(rule ...)` with no name, an unparseable
/// constraint value) is skipped and reported via
/// [`ParsedDesignRules::warnings`] instead of failing the whole parse.
pub fn parse_design_rules(dru_text: &str) -> Result<ParsedDesignRules, ParseDesignRulesError> {
    let exprs = sexpr::parse_all(dru_text)?;
    let mut warnings = Vec::new();
    let mut version = None;
    let mut rules = Vec::new();

    for expr in &exprs {
        match expr.head() {
            Some("version") => match expr.atom(1).and_then(|s| s.parse::<i64>().ok()) {
                Some(v) => version = Some(v),
                None => warnings.push("skipped malformed (version ...) declaration".to_string()),
            },
            Some("rule") => match parse_one_rule(expr, &mut warnings) {
                Some(rule) => rules.push(rule),
                None => warnings.push("skipped a (rule ...) block with no name".to_string()),
            },
            Some(other) => warnings.push(format!("skipped unrecognized top-level form '{other}'")),
            None => {
                warnings.push("skipped a top-level form that wasn't a tagged list".to_string());
            }
        }
    }

    Ok(ParsedDesignRules {
        design_rules: DesignRules { version, rules },
        warnings,
    })
}

fn parse_one_rule(expr: &Sexpr, warnings: &mut Vec<String>) -> Option<Rule> {
    let name = expr.atom(1)?.to_string();

    let layer: Vec<String> = expr
        .find("layer")
        .and_then(Sexpr::as_list)
        .unwrap_or(&[])
        .iter()
        .skip(1)
        .filter_map(Sexpr::as_atom)
        .map(str::to_string)
        .collect();

    let mut constraints = Vec::new();
    for c in expr.find_all("constraint") {
        match parse_one_constraint(c) {
            Some(constraint) => constraints.push(constraint),
            None => warnings.push(format!("rule '{name}': skipped a constraint with no kind")),
        }
    }

    let condition = expr
        .find("condition")
        .and_then(|c| c.atom(1))
        .map(str::to_string);

    Some(Rule {
        name,
        layer,
        constraints,
        condition,
    })
}

fn parse_one_constraint(expr: &Sexpr) -> Option<Constraint> {
    let kind = expr.atom(1)?.to_string();
    let bound = |tag: &str| {
        expr.find(tag)
            .and_then(|b| b.atom(1))
            .and_then(nm_with_unit)
    };
    Some(Constraint {
        kind,
        min_nm: bound("min"),
        max_nm: bound("max"),
        opt_nm: bound("opt"),
    })
}

/// `"0.1mm"` / `"42.5mm"` -> integer nanometres. Only the `mm` suffix has
/// actually been observed (ADR-0002's grounding file uses it exclusively
///  — unlike `.kicad_pcb`, where bare numbers are always implicitly mm,
/// `.kicad_dru` values carry an explicit unit suffix in the same token).
/// Anything else is treated as unparseable rather than guessed at, so a
/// silently-wrong scale factor can never sneak in undetected.
#[allow(clippy::cast_possible_truncation)]
fn nm_with_unit(token: &str) -> Option<i64> {
    let value_str = token.strip_suffix("mm")?;
    let value: f64 = value_str.parse().ok()?;
    Some((value * 1_000_000.0).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim contents of KiCad 10.0.3's shipped
    // /usr/share/kicad/demos/vme-wren/vme-wren.kicad_dru — the same file
    // ADR-0002 ground-truthed the .kicad_dru grammar against.
    const VME_WREN_DRU: &str = r#"
(version 1)

(rule "clearance_under_fpga"
	(constraint clearance (min 0.1mm) )
	(constraint hole_size (min 0.2mm) )
	(constraint via_diameter (min 0.4mm) )
	(condition "A.intersectsArea('underFPGA') || A.intersectsArea('underDDR')" )
)


(rule "zdiff_100R_outer"
	(layer outer)
	(constraint track_width (min 0.115mm) (max 0.115mm) (opt 0.115mm) )
	(constraint diff_pair_gap (min 0.1mm) ( max 1mm) (opt 0.1mm) )
	(constraint diff_pair_uncoupled (max 5mm) )
	(condition "A.inDiffPair('*')" )
)

(rule "zse_50R_outer"
	(layer outer)
	(constraint track_width (min 0.1mm) (max 1mm) (opt 0.2mm) )
	(condition "A.NetClass == 'zse_50r' " )
)

(rule "length_DDR_CMD_FPGA_To_IC13"
	(constraint length (min 42.5mm) (max 43.5mm) (opt 43mm) )
	(condition "A.NetClass == 'DDR4_CMD' && A.fromTo('IC14-*','IC13-*' )" )
)
"#;

    #[test]
    fn parses_the_real_vme_wren_demo_file_with_no_warnings() {
        let parsed = parse_design_rules(VME_WREN_DRU).unwrap();
        assert!(
            parsed.warnings.is_empty(),
            "unexpected warnings: {:?}",
            parsed.warnings
        );
        assert_eq!(parsed.design_rules.version, Some(1));
        assert_eq!(parsed.design_rules.rules.len(), 4);
    }

    #[test]
    fn parses_a_rule_with_no_layer_clause_and_a_boolean_or_condition() {
        let parsed = parse_design_rules(VME_WREN_DRU).unwrap();
        let rule = &parsed.design_rules.rules[0];
        assert_eq!(rule.name, "clearance_under_fpga");
        assert!(rule.layer.is_empty());
        assert_eq!(
            rule.condition.as_deref(),
            Some("A.intersectsArea('underFPGA') || A.intersectsArea('underDDR')")
        );
        assert_eq!(rule.constraints.len(), 3);
        assert_eq!(rule.constraints[0].kind, "clearance");
        assert_eq!(rule.constraints[0].min_nm, Some(100_000));
        assert_eq!(rule.constraints[0].max_nm, None);
    }

    #[test]
    fn parses_a_rule_with_a_layer_clause_and_all_three_bounds() {
        let parsed = parse_design_rules(VME_WREN_DRU).unwrap();
        let rule = &parsed.design_rules.rules[1];
        assert_eq!(rule.name, "zdiff_100R_outer");
        assert_eq!(rule.layer, vec!["outer".to_string()]);

        let track_width = &rule.constraints[0];
        assert_eq!(track_width.kind, "track_width");
        assert_eq!(track_width.min_nm, Some(115_000));
        assert_eq!(track_width.max_nm, Some(115_000));
        assert_eq!(track_width.opt_nm, Some(115_000));

        let uncoupled = &rule.constraints[2];
        assert_eq!(uncoupled.kind, "diff_pair_uncoupled");
        assert_eq!(uncoupled.min_nm, None);
        assert_eq!(uncoupled.max_nm, Some(5_000_000));
    }

    #[test]
    fn parses_a_net_class_equality_condition() {
        let parsed = parse_design_rules(VME_WREN_DRU).unwrap();
        let rule = &parsed.design_rules.rules[2];
        assert_eq!(rule.condition.as_deref(), Some("A.NetClass == 'zse_50r' "));
    }

    #[test]
    fn parses_a_from_to_condition_with_boolean_and() {
        let parsed = parse_design_rules(VME_WREN_DRU).unwrap();
        let rule = &parsed.design_rules.rules[3];
        assert_eq!(
            rule.condition.as_deref(),
            Some("A.NetClass == 'DDR4_CMD' && A.fromTo('IC14-*','IC13-*' )")
        );
        assert_eq!(rule.constraints[0].kind, "length");
        assert_eq!(rule.constraints[0].min_nm, Some(42_500_000));
        assert_eq!(rule.constraints[0].max_nm, Some(43_500_000));
        assert_eq!(rule.constraints[0].opt_nm, Some(43_000_000));
    }

    #[test]
    fn missing_version_and_unrecognized_top_level_forms_are_warned_not_fatal() {
        let parsed = parse_design_rules("(rule \"a\")\n(something_else 1 2)").unwrap();
        assert_eq!(parsed.design_rules.version, None);
        assert_eq!(parsed.design_rules.rules.len(), 1);
        assert_eq!(parsed.warnings.len(), 1);
        assert!(parsed.warnings[0].contains("something_else"));
    }

    #[test]
    fn a_rule_with_no_name_is_skipped_and_warned() {
        let parsed = parse_design_rules("(version 1)\n(rule)").unwrap();
        assert!(parsed.design_rules.rules.is_empty());
        assert_eq!(parsed.warnings.len(), 1);
    }

    #[test]
    fn an_unparseable_unit_suffix_is_skipped_not_guessed_at() {
        let parsed = parse_design_rules(
            r#"(rule "r" (constraint clearance (min 4mil)) (condition "true"))"#,
        )
        .unwrap();
        assert_eq!(parsed.design_rules.rules[0].constraints[0].min_nm, None);
    }

    #[test]
    fn rejects_invalid_sexpr_syntax() {
        assert!(parse_design_rules("(rule \"a\"").is_err());
    }
}
