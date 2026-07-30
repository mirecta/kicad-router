use serde::{Deserialize, Serialize};
use tessera_geom::Polygon;

use crate::layer::LayerId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RuleAreaId(pub u32);

/// A named rule-area zone — KiCad's `(zone (name "...") (keepout ...)
/// (polygon ...))` (`AUTOROUTER_PLAN.md` §7.5.6; `docs/DECISIONS.md`
/// ADR-0002 Q4's empirical grounding, and the "ADR-0002 addendum" entry's
/// `insideArea`/`intersectsArea` findings). This is the geometry/keepout-
/// flag half of what the plan's §7.5.3 calls a `ProtectedRegion` — that
/// larger concept's other two required facets, a net allowlist and scoped
/// constraint overrides, are *derived* from `.kicad_dru` custom-rule
/// conditions that reference this area by `name` (e.g.
/// `A.insideArea('BuckStage') && A.NetClass != 'Power'` implies an
/// allowlist of every net except `Power`) — deriving that needs the
/// custom-rule evaluator, which doesn't exist yet
/// (`tessera_io_kicad::dru_expr` only parses condition text into an AST
/// so far). A `ProtectedRegion` is future work built *from* a `RuleArea`,
/// not this type renamed once the evaluator lands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleArea {
    pub id: RuleAreaId,
    /// The name `.kicad_dru` conditions reference via
    /// `A.insideArea('name')` / `A.intersectsArea('name')`.
    pub name: String,
    pub outline: Polygon,
    pub layers: Vec<LayerId>,
    pub keepout: KeepoutFlags,
}

/// Whether this rule area actually restricts placement of each item kind
/// — mirrors KiCad's own per-zone `RuleAreaSettings` (ADR-0002 Q4). The
/// real `underFPGA`/`underDDR` zones this project has empirically
/// grounded its `.kicad_dru` work against set every one of these to
/// `true` ("allowed") — meaning they're used purely as named regions for
/// custom-rule conditions to reference, not as actual keepouts
/// themselves. Both uses share this same geometry model regardless.
// Five independent, orthogonal flags mirroring KiCad's own flat
// RuleAreaSettings shape exactly (ADR-0002 Q4) — a state-machine/enum
// refactor here would just be reinventing KiCad's own five item-type
// categories under different types, not a real simplification.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeepoutFlags {
    pub tracks_allowed: bool,
    pub vias_allowed: bool,
    pub pads_allowed: bool,
    pub copper_pour_allowed: bool,
    pub footprints_allowed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::Point;

    fn sample_rule_area() -> RuleArea {
        RuleArea {
            id: RuleAreaId(0),
            name: "BuckStage".to_string(),
            outline: Polygon::new(vec![
                Point::new(0, 0),
                Point::new(10_000_000, 0),
                Point::new(10_000_000, 10_000_000),
                Point::new(0, 10_000_000),
            ]),
            layers: vec![LayerId(0), LayerId(1)],
            keepout: KeepoutFlags {
                tracks_allowed: true,
                vias_allowed: true,
                pads_allowed: true,
                copper_pour_allowed: true,
                footprints_allowed: true,
            },
        }
    }

    #[test]
    fn rule_area_serde_roundtrip() {
        let area = sample_rule_area();
        let json = serde_json::to_string(&area).expect("serialize");
        let restored: RuleArea = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, area);
    }

    #[test]
    fn outline_contains_a_point_solidly_inside() {
        let area = sample_rule_area();
        assert!(area
            .outline
            .contains_point(Point::new(5_000_000, 5_000_000)));
    }
}
