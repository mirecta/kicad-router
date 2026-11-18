use std::collections::HashMap;

use tessera_geom::Point;

use crate::board::Board;
use crate::layer::LayerId;
use crate::net::NetId;

/// One endpoint of a [`Connection`]: a pad's position plus the copper
/// layers it's actually present on (a router may land on *any* of them,
/// not just a single fixed layer — relevant once through-hole pads with
/// multiple layers are common, even though M2's fixture/parser scope is
/// 2-layer boards).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub position: Point,
    pub layers: Vec<LayerId>,
}

/// A two-pin connection that still needs a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub net: NetId,
    pub from: Endpoint,
    pub to: Endpoint,
}

/// Result of scanning a board for unrouted connections.
#[derive(Debug, Clone, Default)]
pub struct ConnectionReport {
    /// Two-pin nets, ready to route as-is.
    pub connections: Vec<Connection>,
    /// Nets with three or more pads and no existing geometry: the raw
    /// endpoints, not pre-decomposed into edges. `tessera-model` doesn't
    /// depend on `tessera-global` (crate dependency rule, plan §2.2), so it
    /// can't build the rectilinear Steiner tree itself — that's the
    /// caller's job (`tessera-engine`, which depends on both).
    pub multi_pin_nets: Vec<(NetId, Vec<Endpoint>)>,
    /// Human-readable notes on anything else this scan couldn't classify —
    /// surfaced rather than silently ignored, matching the crate's
    /// no-silent-gaps stance (see `tessera-io-kicad::parser`'s `warnings`
    /// for the same pattern). Currently always empty; kept for whatever
    /// the next unhandled case turns out to be, rather than added ad hoc
    /// later.
    pub skipped: Vec<String>,
}

impl Board {
    /// Finds what still needs routing. A net with no copper of its own yet
    /// (no track, via or zone fill) comes back whole: two-pin nets ready to
    /// route ([`ConnectionReport::connections`]), nets with three or more
    /// pads as raw endpoint groups ([`ConnectionReport::multi_pin_nets`])
    /// for the caller to decompose (see that field's docs for why this
    /// crate can't do it itself).
    ///
    /// A net that already has copper is checked for what it actually
    /// joins (see [`crate::connectivity`]): only the links between its
    /// still separate pad islands come back, as two-pin connections from
    /// the nearest spots of each island — a pad, a via, or a track end. A
    /// fully joined net (by tracks or by a pour) yields nothing.
    #[must_use]
    pub fn find_unrouted_connections(&self) -> ConnectionReport {
        let mut report = ConnectionReport::default();
        let mut nets_with_geometry: std::collections::HashSet<NetId> =
            std::collections::HashSet::new();
        for track in &self.tracks {
            nets_with_geometry.insert(track.net);
        }
        for via in &self.vias {
            nets_with_geometry.insert(via.net);
        }
        for fill in &self.zone_fills {
            nets_with_geometry.insert(fill.net);
        }
        let mut endpoints_by_net: HashMap<NetId, Vec<Endpoint>> = HashMap::new();
        for pad in self.pads.iter().filter(|p| !p.net.is_none()) {
            endpoints_by_net.entry(pad.net).or_default().push(Endpoint {
                position: pad.shape.center(),
                layers: pad.layers.clone(),
            });
        }
        for (net, endpoints) in endpoints_by_net {
            if endpoints.len() < 2 {
                continue; // nothing to connect
            }
            if nets_with_geometry.contains(&net) {
                let islands = crate::connectivity::pad_islands(self, net);
                for (from, to) in crate::connectivity::links_between(&islands) {
                    report.connections.push(Connection { net, from, to });
                }
                continue;
            }
            match <[Endpoint; 2]>::try_from(endpoints) {
                Ok([from, to]) => report.connections.push(Connection { net, from, to }),
                Err(endpoints) => report.multi_pin_nets.push((net, endpoints)),
            }
        }
        // `endpoints_by_net` is a `HashMap`, so the loop above visits nets
        // in an arbitrary, run-to-run-random order (Rust's default hasher
        // is randomly seeded per process) — sorting here is what actually
        // makes that order deterministic, not an incidental side effect.
        // This matters beyond cosmetics: `tessera_engine::route_board`
        // routes `Connection`s in this exact order, committing each one's
        // tracks/vias as real obstacles before the next is attempted, so a
        // random order means a random subset of nets succeeds on any given
        // run — confirmed empirically while investigating task #23's
        // corridor-tuning experiment (routing the same board repeatedly,
        // unchanged code, varied by 22-26 of 37 nets routed run to run).
        // Full scheduling determinism is formally plan §5.4's requirement
        // once parallel rip-up scheduling exists (M5), but this baseline
        // being non-deterministic *today* was never intentional — it's
        // exactly the kind of confound plan §12's empirical rigor is
        // supposed to catch. See `docs/DECISIONS.md`'s corridor-tuning
        // entry.
        report.connections.sort_by_key(|c| c.net); // stable: a net's links keep their tree order
        report.multi_pin_nets.sort_by_key(|(net, _)| *net);

        report
    }

    /// The two pads of a 2-pin net, in board-pad order — used by
    /// `tessera-drc::custom_rules`' single-item checks (`track_width`,
    /// `disallow`, `via_diameter`, `hole_size`) to build a `.kicad_dru`
    /// `fromTo` condition's endpoint facts. `None` for a net with any
    /// other pad count (0, 1, or 3+): a 3+-pad net's Steiner-decomposed
    /// edges aren't tracked on `Board` after routing
    /// (`tessera_engine::route_board`'s decomposition is transient, not
    /// stored), so which specific 2 of N pads a given routed segment
    /// actually connects isn't recoverable from `Board` alone yet — this
    /// deliberately doesn't guess.
    ///
    /// **Not used for `check_length`'s whole-net `fromTo` matching** —
    /// verified against real `kicad-cli` that a `length` constraint's
    /// `fromTo` matches against *any* pad on the net, not just two
    /// designated terminals, so that check builds its endpoint facts from
    /// every pad on the net directly (`tessera-drc`'s
    /// `all_pad_endpoints_for`), with no pin-count restriction — see
    /// `docs/DECISIONS.md`'s "Multi-pin `fromTo`/`length` support" entry.
    #[must_use]
    pub fn two_pin_net_endpoints(&self, net: NetId) -> Option<[&crate::pad::Pad; 2]> {
        let mut pads = self.pads.iter().filter(|p| p.net == net);
        let a = pads.next()?;
        let b = pads.next()?;
        if pads.next().is_some() {
            return None;
        }
        Some([a, b])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::LayerId;
    use crate::net_class::NetClass;
    use crate::pad::{Pad, PadId, PadShape};
    use crate::track::{Track, TrackId};
    use tessera_geom::{Circle, Segment};

    fn board_with_net_classes() -> Board {
        let mut board = Board::new();
        board
            .net_classes
            .insert("Default".to_string(), NetClass::default_placeholder());
        board
    }

    fn pad(id: u32, net: NetId, x: i64, y: i64) -> Pad {
        Pad {
            id: PadId(id),
            shape: PadShape::Circle(Circle::new(Point::new(x, y), 200_000)),
            layers: vec![LayerId(0)],
            net,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        }
    }

    #[test]
    fn two_pin_net_with_no_track_is_unrouted() {
        let mut board = board_with_net_classes();
        let net = NetId(1);
        board.pads.push(pad(0, net, 0, 0));
        board.pads.push(pad(1, net, 1_000_000, 0));

        let report = board.find_unrouted_connections();
        assert_eq!(report.connections.len(), 1);
        assert_eq!(report.connections[0].net, net);
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn net_with_existing_track_is_not_reported() {
        let mut board = board_with_net_classes();
        let net = NetId(1);
        board.pads.push(pad(0, net, 0, 0));
        board.pads.push(pad(1, net, 1_000_000, 0));
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(0, 0), Point::new(1_000_000, 0)),
            width_nm: 250_000,
            layer: LayerId(0),
            net,
            locked: false,
        });

        let report = board.find_unrouted_connections();
        assert!(report.connections.is_empty());
    }

    #[test]
    fn pads_without_a_net_are_never_a_connection_to_route() {
        let mut board = board_with_net_classes();
        board.pads.push(pad(0, NetId::NONE, 0, 0));
        board.pads.push(pad(1, NetId::NONE, 1_000_000, 0));

        let report = board.find_unrouted_connections();
        assert!(report.connections.is_empty());
        assert!(report.multi_pin_nets.is_empty());
    }

    #[test]
    fn single_pad_net_is_ignored() {
        let mut board = board_with_net_classes();
        board.pads.push(pad(0, NetId(1), 0, 0));

        let report = board.find_unrouted_connections();
        assert!(report.connections.is_empty());
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn multi_pin_net_is_reported_with_raw_endpoints_not_dropped_silently() {
        let mut board = board_with_net_classes();
        let net = NetId(1);
        board.pads.push(pad(0, net, 0, 0));
        board.pads.push(pad(1, net, 1_000_000, 0));
        board.pads.push(pad(2, net, 2_000_000, 0));

        let report = board.find_unrouted_connections();
        assert!(report.connections.is_empty());
        assert_eq!(report.multi_pin_nets.len(), 1);
        assert_eq!(report.multi_pin_nets[0].0, net);
        assert_eq!(report.multi_pin_nets[0].1.len(), 3);
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn connections_and_multi_pin_nets_are_sorted_by_net_id() {
        // Deliberately inserted out of NetId order — find_unrouted_
        // connections must sort its output, not just pass through
        // whatever order the underlying HashMap iteration happened to
        // produce (that iteration order is randomized per-process, and
        // route_board routes Connections in this exact order — see this
        // method's docs and docs/DECISIONS.md's corridor-tuning entry
        // for the real run-to-run routing variance this caused before
        // the sort was added).
        let mut board = board_with_net_classes();
        for net_id in [5u32, 1, 3] {
            let net = NetId(net_id);
            board.pads.push(pad(net_id * 10, net, 0, 0));
            board.pads.push(pad(net_id * 10 + 1, net, 1_000_000, 0));
        }
        // A multi-pin net too, also out of order relative to the 2-pin
        // ones above.
        let multi_net = NetId(2);
        board.pads.push(pad(20, multi_net, 0, 0));
        board.pads.push(pad(21, multi_net, 1_000_000, 0));
        board.pads.push(pad(22, multi_net, 2_000_000, 0));

        let report = board.find_unrouted_connections();
        let connection_nets: Vec<u32> = report.connections.iter().map(|c| c.net.0).collect();
        assert_eq!(connection_nets, vec![1, 3, 5]);
        assert_eq!(report.multi_pin_nets[0].0, multi_net);
    }

    #[test]
    fn two_pin_net_endpoints_returns_both_pads() {
        let mut board = board_with_net_classes();
        let net = NetId(1);
        board.pads.push(pad(0, net, 0, 0));
        board.pads.push(pad(1, net, 1_000_000, 0));

        let [a, b] = board.two_pin_net_endpoints(net).expect("2 pads exist");
        assert_eq!(a.id, PadId(0));
        assert_eq!(b.id, PadId(1));
    }

    #[test]
    fn two_pin_net_endpoints_is_none_for_a_three_pad_net() {
        let mut board = board_with_net_classes();
        let net = NetId(1);
        board.pads.push(pad(0, net, 0, 0));
        board.pads.push(pad(1, net, 1_000_000, 0));
        board.pads.push(pad(2, net, 2_000_000, 0));

        assert!(board.two_pin_net_endpoints(net).is_none());
    }

    #[test]
    fn two_pin_net_endpoints_is_none_for_a_single_pad_net() {
        let mut board = board_with_net_classes();
        let net = NetId(1);
        board.pads.push(pad(0, net, 0, 0));

        assert!(board.two_pin_net_endpoints(net).is_none());
    }

    fn track(id: u32, net: NetId, a: (i64, i64), b: (i64, i64)) -> Track {
        Track {
            id: TrackId(id),
            segment: Segment::new(Point::new(a.0, a.1), Point::new(b.0, b.1)),
            width_nm: 250_000,
            layer: LayerId(0),
            net,
            locked: false,
        }
    }

    #[test]
    fn a_partly_routed_net_gets_only_the_missing_link() {
        // pads at x = 0, 1mm, 5mm; a track joins the first two
        let mut board = board_with_net_classes();
        let net = NetId(1);
        board.pads.push(pad(0, net, 0, 0));
        board.pads.push(pad(1, net, 1_000_000, 0));
        board.pads.push(pad(2, net, 5_000_000, 0));
        board.tracks.push(track(0, net, (0, 0), (1_000_000, 0)));

        let report = board.find_unrouted_connections();
        assert!(report.multi_pin_nets.is_empty());
        assert_eq!(report.connections.len(), 1);
        let c = &report.connections[0];
        // from the nearest spot of the routed island to the lone pad
        let ends = [c.from.position, c.to.position];
        assert!(ends.contains(&Point::new(1_000_000, 0)), "{ends:?}");
        assert!(ends.contains(&Point::new(5_000_000, 0)), "{ends:?}");
    }

    #[test]
    fn a_track_that_misses_a_pad_does_not_count_as_routing_it() {
        let mut board = board_with_net_classes();
        let net = NetId(1);
        board.pads.push(pad(0, net, 0, 0));
        board.pads.push(pad(1, net, 3_000_000, 0));
        // a stray stub from pad 0 that stops 1mm short of pad 1
        board.tracks.push(track(0, net, (0, 0), (2_000_000, 0)));

        let report = board.find_unrouted_connections();
        assert_eq!(report.connections.len(), 1);
        let ends = [
            report.connections[0].from.position,
            report.connections[0].to.position,
        ];
        assert!(ends.contains(&Point::new(2_000_000, 0)), "{ends:?}");
    }

    fn fill(net: NetId, x0: i64, x1: i64) -> crate::zone::ZoneFill {
        crate::zone::ZoneFill {
            net,
            layer: LayerId(0),
            clearance_nm: 0,
            polygon: tessera_geom::Polygon::new(vec![
                Point::new(x0, -1_000_000),
                Point::new(x1, -1_000_000),
                Point::new(x1, 1_000_000),
                Point::new(x0, 1_000_000),
            ]),
            inflation_nm: 0,
        }
    }

    #[test]
    fn a_pour_that_covers_every_pad_needs_no_routing() {
        let mut board = board_with_net_classes();
        let net = NetId(1);
        for (i, x) in [0, 2_000_000, 4_000_000].into_iter().enumerate() {
            board.pads.push(pad(u32::try_from(i).unwrap(), net, x, 0));
        }
        board.zone_fills.push(fill(net, -500_000, 4_500_000));

        let report = board.find_unrouted_connections();
        assert!(report.connections.is_empty());
        assert!(report.multi_pin_nets.is_empty());
    }

    #[test]
    fn a_pour_split_into_islands_gets_one_link_between_them() {
        let mut board = board_with_net_classes();
        let net = NetId(1);
        for (i, x) in [0, 1_000_000, 6_000_000, 7_000_000].into_iter().enumerate() {
            board.pads.push(pad(u32::try_from(i).unwrap(), net, x, 0));
        }
        board.zone_fills.push(fill(net, -500_000, 1_500_000));
        board.zone_fills.push(fill(net, 5_500_000, 7_500_000));

        let report = board.find_unrouted_connections();
        assert_eq!(report.connections.len(), 1);
        let ends = [
            report.connections[0].from.position,
            report.connections[0].to.position,
        ];
        assert!(ends.contains(&Point::new(1_000_000, 0)), "{ends:?}");
        assert!(ends.contains(&Point::new(6_000_000, 0)), "{ends:?}");
    }
}
