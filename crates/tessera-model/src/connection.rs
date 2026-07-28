use std::collections::HashMap;

use tessera_geom::Point;

use crate::board::Board;
use crate::net::NetId;
use crate::pad::PadShape;

/// A two-pin connection that still needs a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Connection {
    pub net: NetId,
    pub from: Point,
    pub to: Point,
}

/// Result of scanning a board for unrouted connections.
#[derive(Debug, Clone, Default)]
pub struct ConnectionReport {
    pub connections: Vec<Connection>,
    /// Human-readable notes on nets this scan couldn't classify — surfaced
    /// rather than silently ignored, matching the crate's no-silent-gaps
    /// stance (see `tessera-io-kicad::parser`'s `warnings` for the same
    /// pattern).
    pub skipped: Vec<String>,
}

impl Board {
    /// Finds nets that need routing: exactly two pads, no existing track or
    /// via on that net yet.
    ///
    /// Scoped to two-pin nets only — multi-pin net decomposition (a
    /// rectilinear Steiner tree per net, plan §5.1) is M3 scope (FLUTE)
    /// and isn't implemented. Nets with more than two pads are reported in
    /// [`ConnectionReport::skipped`], not silently dropped or
    /// mis-routed as a single 2-pin connection between two arbitrary pads.
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

        let mut pad_positions_by_net: HashMap<NetId, Vec<Point>> = HashMap::new();
        for pad in &self.pads {
            let PadShape::Circle(circle) = &pad.shape;
            pad_positions_by_net
                .entry(pad.net)
                .or_default()
                .push(circle.center);
        }

        for (net, positions) in pad_positions_by_net {
            if nets_with_geometry.contains(&net) {
                continue;
            }
            match positions.len() {
                0 | 1 => {} // nothing to connect
                2 => report.connections.push(Connection {
                    net,
                    from: positions[0],
                    to: positions[1],
                }),
                n => report.skipped.push(format!(
                    "net {net:?} has {n} pads — multi-pin net decomposition isn't implemented yet (M3 scope)"
                )),
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
    fn multi_pin_net_is_reported_as_skipped_not_dropped_silently() {
        let mut board = board_with_net_classes();
        let net = NetId(1);
        board.pads.push(pad(0, net, 0, 0));
        board.pads.push(pad(1, net, 1_000_000, 0));
        board.pads.push(pad(2, net, 2_000_000, 0));

        let report = board.find_unrouted_connections();
        assert!(report.connections.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }
}
