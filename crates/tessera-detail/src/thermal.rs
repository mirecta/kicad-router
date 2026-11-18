//! Room for thermal spokes. A pad that joins a copper zone of its own net
//! does so through spokes across the zone's thermal gap; a track of another
//! net passing right next to the pad cuts them, and KiCad reports
//! `starved_thermal` (too few spokes). Forbidding those spots would block
//! the track between two 2.54 mm header pins, so they are only made dearer:
//! a route goes around when it reasonably can.

use tessera_geom::{Point, Segment};
use tessera_model::{Board, LayerId, NetId, PadShape};

use crate::grid::{Cell, GridBounds};

/// Extra cost per 0.1 mm step inside a spoke's corridor — two or three
/// steps there cost about as much as half a millimetre of detour.
pub(crate) const SPOKE_COST: i64 = 2_000;

/// Where, in one routing window, routing `net` would crowd another net's
/// thermal spokes, per signal layer index.
pub(crate) struct SpokeField {
    bounds: GridBounds,
    cells: Vec<Vec<bool>>,
}

impl SpokeField {
    /// `margin_nm`: how far the routed track's centreline must stay from a
    /// spoke edge (its half-width plus clearance).
    pub(crate) fn build(
        board: &Board,
        net: NetId,
        bounds: GridBounds,
        layers: &[LayerId],
        margin_nm: i64,
    ) -> Self {
        let n = usize::try_from(bounds.width.max(0)).unwrap_or(0)
            * usize::try_from(bounds.height.max(0)).unwrap_or(0);
        let mut cells = vec![Vec::new(); layers.len()];
        let lo = bounds.origin;
        let hi = Point::new(
            lo.x + i64::from(bounds.width) * crate::grid::CELL_NM,
            lo.y + i64::from(bounds.height) * crate::grid::CELL_NM,
        );
        for zone in board
            .copper_zones
            .iter()
            .filter(|z| z.net != net && !z.net.is_none())
        {
            let reach = zone.thermal_gap_nm + zone.min_thickness_nm.max(250_000);
            let half_width = zone.thermal_bridge_nm / 2 + margin_nm;
            for pad in board.pads.iter().filter(|p| p.net == zone.net) {
                let c = pad.shape.center();
                if zone.outline.as_ref().is_some_and(|o| !o.contains_point(c)) {
                    continue;
                }
                let (hx, hy) = pad.shape.half_extents();
                let round = matches!(pad.shape, PadShape::Circle(_));
                let ext = hx.max(hy) + reach + half_width;
                if c.x + ext < lo.x || c.x - ext > hi.x || c.y + ext < lo.y || c.y - ext > hi.y {
                    continue;
                }
                // KiCad's default spoke angles: 45° on round pads, 0/90 otherwise
                let ends: Vec<Point> = if round {
                    let d = (hx + reach) * 7071 / 10_000;
                    vec![
                        Point::new(c.x + d, c.y + d),
                        Point::new(c.x - d, c.y + d),
                        Point::new(c.x + d, c.y - d),
                        Point::new(c.x - d, c.y - d),
                    ]
                } else {
                    vec![
                        Point::new(c.x + hx + reach, c.y),
                        Point::new(c.x - hx - reach, c.y),
                        Point::new(c.x, c.y + hy + reach),
                        Point::new(c.x, c.y - hy - reach),
                    ]
                };
                for (li, layer) in layers.iter().enumerate() {
                    if !(zone.layers.contains(layer) && pad.layers.contains(layer)) {
                        continue;
                    }
                    if cells[li].is_empty() {
                        cells[li] = vec![false; n];
                    }
                    for &end in &ends {
                        mark(&mut cells[li], bounds, Segment::new(c, end), half_width);
                    }
                }
            }
        }
        Self { bounds, cells }
    }

    pub(crate) fn cost(&self, cell: Cell, layer_index: usize) -> i64 {
        let Some(layer) = self.cells.get(layer_index).filter(|l| !l.is_empty()) else {
            return 0;
        };
        if cell.x < 0 || cell.y < 0 || cell.x >= self.bounds.width || cell.y >= self.bounds.height {
            return 0;
        }
        let i = usize::try_from(cell.y).unwrap_or(0)
            * usize::try_from(self.bounds.width).unwrap_or(0)
            + usize::try_from(cell.x).unwrap_or(0);
        if layer.get(i).copied().unwrap_or(false) {
            SPOKE_COST
        } else {
            0
        }
    }
}

fn mark(cells: &mut [bool], bounds: GridBounds, spoke: Segment, half_width: i64) {
    let cell = crate::grid::CELL_NM;
    let (o, w, h) = (bounds.origin, bounds.width, bounds.height);
    let x0 = (spoke.a.x.min(spoke.b.x) - half_width - o.x)
        .div_euclid(cell)
        .max(0);
    let x1 =
        ((spoke.a.x.max(spoke.b.x) + half_width - o.x).div_euclid(cell) + 1).min(i64::from(w) - 1);
    let y0 = (spoke.a.y.min(spoke.b.y) - half_width - o.y)
        .div_euclid(cell)
        .max(0);
    let y1 =
        ((spoke.a.y.max(spoke.b.y) + half_width - o.y).div_euclid(cell) + 1).min(i64::from(h) - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let p = Point::new(o.x + x * cell, o.y + y * cell);
            if !spoke.clears_point(p, half_width) {
                if let Ok(i) = usize::try_from(y * i64::from(w) + x) {
                    if let Some(c) = cells.get_mut(i) {
                        *c = true;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::{Circle, Polygon, Rect};
    use tessera_model::{CopperZone, Pad, PadId};

    fn board_with_gnd_pad(shape: PadShape) -> Board {
        let mut board = Board::new();
        board.copper_zones.push(CopperZone {
            net: NetId(1),
            layers: vec![LayerId(0)],
            outline: Some(Polygon::new(vec![
                Point::new(-10_000_000, -10_000_000),
                Point::new(10_000_000, -10_000_000),
                Point::new(10_000_000, 10_000_000),
                Point::new(-10_000_000, 10_000_000),
            ])),
            thermal_gap_nm: 500_000,
            thermal_bridge_nm: 500_000,
            min_thickness_nm: 250_000,
        });
        board.pads.push(Pad {
            id: PadId(0),
            shape,
            layers: vec![LayerId(0)],
            net: NetId(1),
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });
        board
    }

    fn bounds() -> GridBounds {
        GridBounds {
            origin: Point::new(-5_000_000, -5_000_000),
            width: 101,
            height: 101,
        }
    }

    fn at(x: i64, y: i64) -> Cell {
        Cell::from_point(Point::new(x, y), bounds().origin)
    }

    #[test]
    fn rect_pads_get_axis_spokes_other_nets_pay_for() {
        let board = board_with_gnd_pad(PadShape::Rect(Rect::new(
            Point::new(0, 0),
            800_000,
            800_000,
        )));
        let f = SpokeField::build(
            &board,
            NetId(2),
            bounds(),
            &[LayerId(0), LayerId(1)],
            200_000,
        );
        assert_eq!(f.cost(at(1_200_000, 0), 0), SPOKE_COST); // across the gap, east
        assert_eq!(f.cost(at(0, -1_200_000), 0), SPOKE_COST); // north
        assert_eq!(f.cost(at(1_200_000, 1_200_000), 0), 0); // a corner: no spoke
        assert_eq!(f.cost(at(1_200_000, 0), 1), 0); // other layer, no zone there
        assert_eq!(f.cost(at(3_000_000, 0), 0), 0); // beyond the spoke
                                                    // the pad's own net is free to go there
        let own = SpokeField::build(&board, NetId(1), bounds(), &[LayerId(0)], 200_000);
        assert_eq!(own.cost(at(1_200_000, 0), 0), 0);
    }

    #[test]
    fn round_pads_get_diagonal_spokes() {
        let board = board_with_gnd_pad(PadShape::Circle(Circle::new(Point::new(0, 0), 800_000)));
        let f = SpokeField::build(&board, NetId(2), bounds(), &[LayerId(0)], 200_000);
        assert_eq!(f.cost(at(900_000, 900_000), 0), SPOKE_COST);
        assert_eq!(f.cost(at(1_300_000, 0), 0), 0);
    }
}
