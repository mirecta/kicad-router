use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::layer::Layer;
use crate::net::{Net, NetId};
use crate::net_class::NetClass;
use crate::pad::Pad;
use crate::track::Track;
use crate::via::Via;

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
        let net = self.nets.get(&net_id)?;
        self.net_classes.get(&net.net_class)
    }
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
