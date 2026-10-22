use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use tessera_geom::Polygon;

use crate::design_rules::DesignRules;
use crate::hole::Hole;
use crate::layer::{Layer, LayerId};
use crate::net::{Net, NetId};
use crate::net_class::NetClass;
use crate::pad::Pad;
use crate::rule_area::RuleArea;
use crate::track::Track;
use crate::via::Via;
use crate::zone::{CopperZone, Plane, ZoneFill};

/// The whole board: everything `tessera-drc`/`tessera-engine` need, ingested
/// in one bulk fetch per plan §2.3 — nothing in this crate ever queries
/// KiCad again after construction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Board {
    pub layers: Vec<Layer>,
    pub nets: HashMap<NetId, Net>,
    pub net_classes: HashMap<String, NetClass>,
    pub tracks: Vec<Track>,
    pub vias: Vec<Via>,
    pub pads: Vec<Pad>,
    /// Named rule-area zones — see [`RuleArea`]'s docs for what this
    /// crate does and doesn't model yet (geometry/keepout flags only, not
    /// the fuller `ProtectedRegion` the plan describes).
    pub rule_areas: Vec<RuleArea>,
    /// The board's physical outline (the `Edge.Cuts` layer), as a single
    /// closed polygon — `None` if the board has no outline at all, or if
    /// `tessera-io-kicad` couldn't assemble one (e.g. it contains an arc
    /// segment; see that crate's parser docs for why this doesn't
    /// approximate arcs rather than guess at an inexact polygon). A
    /// multi-outline board (a cutout/inner boundary, or genuinely
    /// disjoint board pieces) isn't represented either — only ever the
    /// single closed loop a straight-edged rectangular-ish board has.
    pub outline: Option<Polygon>,
    /// Non-plated holes (mounting holes etc.) — see [`Hole`].
    pub npth_holes: Vec<Hole>,
    /// Existing copper-zone fills — see [`ZoneFill`].
    pub zone_fills: Vec<ZoneFill>,
    /// Every copper zone in the file, filled or not.
    pub copper_zones: Vec<CopperZone>,
    /// Layers reserved as planes — set by the caller (it's a routing
    /// choice, not something the board file states).
    pub planes: Vec<Plane>,
    pub rules: DesignRules,
}

impl Board {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The effective net class for a net, or `None` if either the net or
    /// its assigned class name is missing from this board. Callers should
    /// treat a missing net class as a data-integrity error to surface, not
    /// silently fall back to some default — every real KiCad net always
    /// resolves to at least `"Default"`.
    #[must_use]
    pub fn net_class_for(&self, net_id: NetId) -> Option<&NetClass> {
        if net_id.is_none() {
            // Verified against real `kicad-cli`: no-net copper is held to
            // the Default class's clearance.
            return self.net_classes.get("Default");
        }
        let net = self.nets.get(&net_id)?;
        self.net_classes.get(&net.net_class)
    }

    /// The clearance required between two items on `net_a` and `net_b`, or
    /// `None` if no clearance check applies at all (same net — electrically
    /// connected items are allowed to touch) or if either net's class is
    /// missing from the board (a data-integrity problem for the caller to
    /// surface, not something to paper over with a default here).
    ///
    /// KiCad's rule (per the constraint-resolution order in plan §3.1, and
    /// consistent with the `RULE_RESOLVER`/`CONSTRAINT_TYPE` interface read
    /// from PNS during the M0 survey — see `docs/PRIOR_ART.md`): when two
    /// different net classes disagree, the more restrictive (larger)
    /// clearance applies. This implements only the net-class layer of that
    /// precedence chain (board-level minimums and custom-rule/local
    /// overrides aren't modelled yet) — verified against the M1 parity
    /// harness (`crates/tessera-io-kicad/tests/drc_parity.rs`) for the
    /// track-track case.
    ///
    /// Lives here (not in `tessera-drc`) because it's a pure board-data
    /// query every crate that needs "how far apart must these two nets be"
    /// can use without pulling in `tessera-drc`'s violation-checking
    /// machinery — in particular `tessera-detail`'s router, which per the
    /// crate dependency rule (plan §2.2) depends on `geom`+`model` only.
    /// `net`'s differential-pair partner, by KiCad's naming rule — verified
    /// against real `kicad-cli`: from the end of the name, skip digits and
    /// underscores; if the next character is `P`, `N`, `+` or `-`, the
    /// partner is the same name with just that character swapped (`P`↔`N`,
    /// `+`↔`-`) — so `SIG_P`/`SIG_N`, `//DP`/`//DN`, `DP1`/`DN1`,
    /// `U+_7`/`U-_7`, but not `XP9`/`XN8`, `CLKP`/`CLKM` or `dp`/`dn`.
    #[must_use]
    pub fn diff_pair_partner(&self, net: NetId) -> Option<NetId> {
        let name = &self.nets.get(&net)?.name;
        let partner = diff_pair_partner_name(name)?;
        self.nets.values().find(|n| n.name == partner).map(|n| n.id)
    }

    /// The track width new routing of `net` uses: the class's
    /// differential-pair width for a net in a pair (when the class sets one),
    /// otherwise its track width.
    #[must_use]
    pub fn track_width_for(&self, net: NetId) -> Option<i64> {
        let class = self.net_class_for(net)?;
        let in_pair = self.diff_pair_partner(net).is_some();
        Some(match class.diff_pair_track_width_nm {
            Some(width) if in_pair => width,
            _ => class.track_width_nm,
        })
    }

    /// Copper layers signals may be routed on: every layer except planes.
    #[must_use]
    pub fn signal_layers(&self) -> Vec<LayerId> {
        self.layers
            .iter()
            .map(|l| l.id)
            .filter(|id| !self.planes.iter().any(|p| p.layer == *id))
            .collect()
    }

    #[must_use]
    pub fn is_plane_net(&self, net: NetId) -> bool {
        self.planes.iter().any(|p| p.net == net)
    }

    /// `(diameter, drill)` for a via newly routed on `net`: the net
    /// class's own sizes, with the diameter enlarged if that's needed for
    /// the annular ring to meet the board's `min_via_annular_width` — a
    /// router must never emit a via its own board's rules reject. Real
    /// case: a project whose Default class says 0.6/0.3mm on a board
    /// requiring a 0.2mm ring gets 0.7/0.3mm.
    #[must_use]
    pub fn via_size_for(&self, net: NetId) -> Option<(i64, i64)> {
        let class = self.net_class_for(net)?;
        let drill_nm = class.via_drill_nm;
        let diameter_nm = class
            .via_diameter_nm
            .max(drill_nm + 2 * self.rules.min_via_annular_width_nm);
        Some((diameter_nm, drill_nm))
    }

    /// The clearance required between `fill` and copper on `other_net`:
    /// the larger of the zone's own clearance and the net-class one
    /// (verified against real `kicad-cli`: zone 0.25 vs class 0.2 → 0.25,
    /// zone 0.1 vs class 0.3 → 0.3), or `None` on the zone's own net.
    #[must_use]
    pub fn zone_clearance_nm(&self, fill: &ZoneFill, other_net: NetId) -> Option<i64> {
        let class_nm = self.resolved_clearance_nm(fill.net, other_net)?;
        Some(class_nm.max(fill.clearance_nm))
    }

    #[must_use]
    pub fn resolved_clearance_nm(&self, net_a: NetId, net_b: NetId) -> Option<i64> {
        if net_a.same_net(net_b) {
            return None;
        }
        let a = self.net_class_for(net_a)?;
        let b = self.net_class_for(net_b)?;
        Some(a.clearance_nm.max(b.clearance_nm))
    }
}

/// The name of the partner net `name` would pair with — see
/// [`Board::diff_pair_partner`].
#[must_use]
pub fn diff_pair_partner_name(name: &str) -> Option<String> {
    let trailing = name
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '_')
        .count();
    let split = name.len() - trailing; // the skipped characters are ASCII
    let (head, tail) = name.split_at(split);
    let mut head_chars = head.chars();
    let swapped = match head_chars.next_back()? {
        'P' => 'N',
        'N' => 'P',
        '+' => '-',
        '-' => '+',
        _ => return None,
    };
    Some(format!("{}{swapped}{tail}", head_chars.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{Layer, LayerId};
    use crate::net::Net;
    use crate::net_class::NetClass;
    use crate::pad::{Pad, PadId, PadShape};
    use crate::track::{Track, TrackId};
    use tessera_geom::{Circle, Point, Segment};

    fn sample_board() -> Board {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
        board.layers.push(Layer::copper(LayerId(1), "B.Cu"));

        board
            .net_classes
            .insert("Default".to_string(), NetClass::default_placeholder());

        let net_id = NetId(1);
        board.nets.insert(
            net_id,
            Net {
                id: net_id,
                name: "GND".to_string(),
                net_class: "Default".to_string(),
            },
        );

        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(0, 0), Point::new(1_000_000, 0)),
            width_nm: 250_000,
            layer: LayerId(0),
            net: net_id,
            locked: false,
        });

        board.pads.push(Pad {
            id: PadId(0),
            shape: PadShape::Circle(Circle::new(Point::new(0, 0), 400_000)),
            layers: vec![LayerId(0)],
            net: net_id,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });

        board
    }

    #[test]
    fn net_class_lookup_resolves() {
        let board = sample_board();
        let class = board.net_class_for(NetId(1)).expect("GND has a class");
        assert_eq!(class.name, "Default");
    }

    #[test]
    fn no_net_copper_uses_the_default_class_and_is_never_the_same_net() {
        let board = sample_board();
        let default = board.net_classes["Default"].clearance_nm;
        assert_eq!(
            board.resolved_clearance_nm(NetId::NONE, NetId::NONE),
            Some(default)
        );
        assert!(NetId(1).same_net(NetId(1)));
        assert!(!NetId::NONE.same_net(NetId::NONE));
    }

    #[test]
    fn via_size_is_enlarged_only_when_the_annular_ring_would_be_too_small() {
        let mut board = sample_board();
        let class = board.net_classes.get_mut("Default").unwrap();
        class.via_diameter_nm = 600_000;
        class.via_drill_nm = 300_000;
        board.rules.min_via_annular_width_nm = 200_000;
        assert_eq!(board.via_size_for(NetId(1)), Some((700_000, 300_000)));
        board.rules.min_via_annular_width_nm = 100_000;
        assert_eq!(board.via_size_for(NetId(1)), Some((600_000, 300_000)));
    }

    #[test]
    fn diff_pair_names_follow_kicads_rule() {
        let pairs = [
            ("SIG_P", "SIG_N"),
            ("//DP", "//DN"),
            ("DP1", "DN1"),
            ("A_P_1", "A_N_1"),
            ("D+", "D-"),
            ("U-_7", "U+_7"),
            ("P", "N"),
        ];
        for (a, b) in pairs {
            assert_eq!(diff_pair_partner_name(a).as_deref(), Some(b), "{a}");
        }
        for not_a_pair in ["CLKM", "dp", "USB", "XP9_x"] {
            assert_eq!(diff_pair_partner_name(not_a_pair), None, "{not_a_pair}");
        }
        // XP9 would pair with XN9 — XN8 isn't its partner.
        assert_eq!(diff_pair_partner_name("XP9").as_deref(), Some("XN9"));
    }

    #[test]
    fn net_class_lookup_missing_net_is_none() {
        let board = sample_board();
        assert!(board.net_class_for(NetId(999)).is_none());
    }

    #[test]
    fn board_serde_roundtrip() {
        let board = sample_board();
        let json = serde_json::to_string(&board).expect("serialize");
        let restored: Board = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.tracks.len(), board.tracks.len());
        assert_eq!(restored.pads.len(), board.pads.len());
        assert_eq!(
            restored.net_class_for(NetId(1)).unwrap().name,
            board.net_class_for(NetId(1)).unwrap().name
        );
    }
}
