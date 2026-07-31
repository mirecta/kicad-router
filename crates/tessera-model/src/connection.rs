use std::collections::HashMap;

use tessera_geom::Point;

use crate::board::Board;
use crate::layer::LayerId;
use crate::net::NetId;
use crate::pad::PadShape;

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
    /// Finds nets that need routing: two or more pads, no existing track or
    /// via on that net yet. Two-pin nets come back ready to route
    /// ([`ConnectionReport::connections`]); nets with three or more pads
    /// come back as raw endpoint groups
    /// ([`ConnectionReport::multi_pin_nets`]) for the caller to decompose
    /// (see that field's docs for why this crate can't do it itself).
    ///
    /// A net already carrying any track or via is treated as already
    /// routed and excluded entirely — this doesn't verify the existing
    /// geometry actually *connects* the pads (that needs a connectivity
    /// check this crate doesn't have yet), so a net with a stray
    /// disconnected track fragment would be incorrectly skipped. Acceptable
    /// for M2's scope (trivial boards, no partial-routing edge cases);
    /// revisit once partial-routing (plan §7.5.5) is in scope.
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

        let mut endpoints_by_net: HashMap<NetId, Vec<Endpoint>> = HashMap::new();
        for pad in &self.pads {
            let PadShape::Circle(circle) = &pad.shape;
            endpoints_by_net.entry(pad.net).or_default().push(Endpoint {
                position: circle.center,
                layers: pad.layers.clone(),
            });
        }

        for (net, endpoints) in endpoints_by_net {
            if nets_with_geometry.contains(&net) {
                continue;
            }
            match <[Endpoint; 2]>::try_from(endpoints) {
                Ok([from, to]) => report.connections.push(Connection { net, from, to }),
                Err(endpoints) if endpoints.len() <= 1 => {} // nothing to connect
                Err(endpoints) => report.multi_pin_nets.push((net, endpoints)),
            }
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::LayerId;
    use crate::net_class::NetClass;
    use crate::pad::{Pad, PadId};
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
}
