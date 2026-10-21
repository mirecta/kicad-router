use serde::{Deserialize, Serialize};
use tessera_model::{Board, LayerId, Pad, PadShape, Via, ZoneFill};

use crate::clearance::{verdict_threshold_nm, via_circle};
use crate::violation::ItemRef;

/// Copper too close to another net's existing zone fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneClearanceViolation {
    pub item: ItemRef,
    /// Index into `Board::zone_fills`.
    pub fill: usize,
    pub required_nm: i64,
}

/// Every track, via and pad closer to another net's zone fill (on the
/// same layer) than [`Board::zone_clearance_nm`] allows, with KiCad's
/// 500nm clearance tolerance — all verified against real `kicad-cli`.
#[must_use]
pub fn check_zone_clearance(board: &Board) -> Vec<ZoneClearanceViolation> {
    let mut violations = Vec::new();
    for (index, fill) in board.zone_fills.iter().enumerate() {
        let mut check = |item, net, clears: &dyn Fn(i64) -> bool| {
            if let Some(required_nm) = board.zone_clearance_nm(fill, net) {
                if !clears(verdict_threshold_nm(required_nm)) {
                    violations.push(ZoneClearanceViolation {
                        item,
                        fill: index,
                        required_nm,
                    });
                }
            }
        };
        for t in board.tracks.iter().filter(|t| t.layer == fill.layer) {
            check(ItemRef::Track(t.id), t.net, &|min| {
                fill.polygon
                    .clears_segment(t.segment, min + fill.inflation_nm + t.width_nm / 2)
            });
        }
        for v in board.vias.iter().filter(|v| via_spans(v, fill.layer)) {
            check(ItemRef::Via(v.id), v.net, &|min| {
                fill.polygon
                    .clears_circle(via_circle(v), min + fill.inflation_nm)
            });
        }
        for p in board.pads.iter().filter(|p| p.layers.contains(&fill.layer)) {
            check(ItemRef::Pad(p.id), p.net, &|min| {
                pad_clears_fill(p, fill, min + fill.inflation_nm)
            });
        }
    }
    violations
}

fn via_spans(via: &Via, layer: LayerId) -> bool {
    let lo = via.from_layer.0.min(via.to_layer.0);
    let hi = via.from_layer.0.max(via.to_layer.0);
    (lo..=hi).contains(&layer.0)
}

fn pad_clears_fill(pad: &Pad, fill: &ZoneFill, min_nm: i64) -> bool {
    match &pad.shape {
        PadShape::Circle(c) => fill.polygon.clears_circle(*c, min_nm),
        PadShape::Rect(r) => fill.polygon.clears_rect(*r, min_nm),
        PadShape::Oval(o) => fill.polygon.clears_segment(o.spine, min_nm + o.radius_nm),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::{Point, Polygon, Segment};
    use tessera_model::{NetClass, NetId, Track, TrackId};

    fn board_with_fill_and_track(gap_nm: i64, zone_clearance_nm: i64) -> Board {
        let mut board = Board::new();
        let mut class = NetClass::default_placeholder();
        class.clearance_nm = 200_000;
        board.net_classes.insert("Default".to_string(), class);
        for id in [1, 2] {
            board.nets.insert(
                NetId(id),
                tessera_model::Net {
                    id: NetId(id),
                    name: format!("N{id}"),
                    net_class: "Default".to_string(),
                },
            );
        }
        board.zone_fills.push(ZoneFill {
            net: NetId(2),
            layer: LayerId(0),
            clearance_nm: zone_clearance_nm,
            inflation_nm: 0,
            polygon: Polygon::new(vec![
                Point::new(0, 0),
                Point::new(5_000_000, 0),
                Point::new(5_000_000, 5_000_000),
                Point::new(0, 5_000_000),
            ]),
        });
        let x = 5_000_000 + gap_nm + 100_000;
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(x, 1_000_000), Point::new(x, 4_000_000)),
            width_nm: 200_000,
            layer: LayerId(0),
            net: NetId(1),
            locked: false,
        });
        board
    }

    #[test]
    fn the_larger_of_zone_and_class_clearance_applies_with_kicads_tolerance() {
        assert!(
            check_zone_clearance(&board_with_fill_and_track(250_000 - 500, 250_000)).is_empty()
        );
        assert_eq!(
            check_zone_clearance(&board_with_fill_and_track(250_000 - 501, 250_000)).len(),
            1
        );
        // Zone 0.1 vs class 0.2: the class governs.
        assert_eq!(
            check_zone_clearance(&board_with_fill_and_track(150_000, 100_000)).len(),
            1
        );
    }

    #[test]
    fn the_zones_own_net_is_never_a_violation() {
        let mut board = board_with_fill_and_track(0, 250_000);
        board.tracks[0].net = NetId(2);
        assert!(check_zone_clearance(&board).is_empty());
    }
}
