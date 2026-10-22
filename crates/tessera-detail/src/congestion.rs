//! Negotiated congestion (`PathFinder`, McMurchie & Ebeling) at the detail
//! level: every net is routed against the board's fixed geometry only, and
//! the other nets' routes are a *cost* rather than an obstacle — a cost that
//! grows round by round wherever nets keep colliding, until they spread out.

use std::collections::{HashMap, HashSet};

use tessera_geom::{Point, Segment};
use tessera_model::{Board, LayerId, NetId};

use crate::grid::CELL_NM;
use crate::router::RoutedPath;

/// Board-wide congestion state on the signal layers (in
/// `Board::signal_layers` order, the order the router indexes layers by).
#[derive(Debug, Clone)]
pub struct Congestion {
    origin: Point,
    width: i64,
    height: i64,
    layers: Vec<LayerId>,
    /// How many nets' footprints cover each cell (a net counts once).
    occupancy: Vec<u16>,
    /// Accumulated cost of past collisions at each cell.
    history: Vec<i64>,
    /// Cost per covering net, raised every round.
    present_factor: i64,
    /// Cells each net's footprint covers, and its routes' centre lines.
    footprints: HashMap<NetId, Vec<usize>>,
    centre_lines: HashMap<NetId, Vec<usize>>,
}

impl Congestion {
    /// Covers the board outline (or, without one, every pad) with a margin.
    #[must_use]
    pub fn new(board: &Board, present_factor: i64) -> Option<Self> {
        let points: Vec<Point> = match &board.outline {
            Some(outline) => outline.vertices.clone(),
            None => board.pads.iter().map(|p| p.shape.center()).collect(),
        };
        let margin = 8_000_000;
        let min_x = points.iter().map(|p| p.x).min()? - margin;
        let min_y = points.iter().map(|p| p.y).min()? - margin;
        let max_x = points.iter().map(|p| p.x).max()? + margin;
        let max_y = points.iter().map(|p| p.y).max()? + margin;
        let width = (max_x - min_x) / CELL_NM + 1;
        let height = (max_y - min_y) / CELL_NM + 1;
        let layers = board.signal_layers();
        let len = usize::try_from(width * height).ok()? * layers.len();
        Some(Self {
            origin: Point::new(min_x, min_y),
            width,
            height,
            layers,
            occupancy: vec![0; len],
            history: vec![0; len],
            present_factor,
            footprints: HashMap::new(),
            centre_lines: HashMap::new(),
        })
    }

    fn index(&self, p: Point, layer_index: usize) -> Option<usize> {
        let x = (p.x - self.origin.x).div_euclid(CELL_NM);
        let y = (p.y - self.origin.y).div_euclid(CELL_NM);
        if x < 0 || y < 0 || x >= self.width || y >= self.height || layer_index >= self.layers.len()
        {
            return None;
        }
        let planar = usize::try_from(y * self.width + x).ok()?;
        Some(planar * self.layers.len() + layer_index)
    }

    /// The extra cost of a route passing through `p` on the `layer_index`-th
    /// signal layer: history plus the present factor per covering net.
    #[must_use]
    pub fn cost(&self, p: Point, layer_index: usize) -> i64 {
        self.index(p, layer_index).map_or(0, |i| {
            self.history[i] + self.present_factor * i64::from(self.occupancy[i])
        })
    }

    /// Every cell within `radius_nm` of `segment` on `layer_index`.
    fn cells_near(
        &self,
        segment: Segment,
        layer_index: usize,
        radius_nm: i64,
        out: &mut HashSet<usize>,
    ) {
        let (lo_x, hi_x) = (
            segment.a.x.min(segment.b.x) - radius_nm,
            segment.a.x.max(segment.b.x) + radius_nm,
        );
        let (lo_y, hi_y) = (
            segment.a.y.min(segment.b.y) - radius_nm,
            segment.a.y.max(segment.b.y) + radius_nm,
        );
        let mut y = lo_y - (lo_y - self.origin.y).rem_euclid(CELL_NM);
        while y <= hi_y {
            let mut x = lo_x - (lo_x - self.origin.x).rem_euclid(CELL_NM);
            while x <= hi_x {
                let p = Point::new(x, y);
                if !segment.clears_point(p, radius_nm) {
                    out.extend(self.index(p, layer_index));
                }
                x += CELL_NM;
            }
            y += CELL_NM;
        }
    }

    /// Removes `net`'s footprint (before rerouting it).
    pub fn clear_net(&mut self, net: NetId) {
        for i in self.footprints.remove(&net).unwrap_or_default() {
            self.occupancy[i] = self.occupancy[i].saturating_sub(1);
        }
        self.centre_lines.remove(&net);
    }

    /// Records `net`'s routes: its footprint covers every cell where another
    /// net's track centre line would come closer than clearance plus both
    /// half-widths (`track_reach_nm`), or, around a via, `via_reach_nm` on
    /// every layer.
    pub fn set_net(
        &mut self,
        net: NetId,
        paths: &[&RoutedPath],
        track_reach_nm: i64,
        via_reach_nm: i64,
    ) {
        self.clear_net(net);
        let mut footprint = HashSet::new();
        let mut centre = HashSet::new();
        for path in paths {
            for (segment, layer) in &path.segments {
                let Some(li) = self.layers.iter().position(|l| l == layer) else {
                    continue;
                };
                self.cells_near(*segment, li, track_reach_nm, &mut footprint);
                self.cells_near(*segment, li, CELL_NM / 2, &mut centre);
            }
            for &v in &path.vias {
                for li in 0..self.layers.len() {
                    self.cells_near(Segment::new(v, v), li, via_reach_nm, &mut footprint);
                    self.cells_near(Segment::new(v, v), li, CELL_NM / 2, &mut centre);
                }
            }
        }
        for &i in &footprint {
            self.occupancy[i] = self.occupancy[i].saturating_add(1);
        }
        self.footprints.insert(net, footprint.into_iter().collect());
        self.centre_lines.insert(net, centre.into_iter().collect());
    }

    /// True iff some route of `net` runs through another net's footprint —
    /// the nets worth re-planning next round.
    #[must_use]
    pub fn collides(&self, net: NetId) -> bool {
        self.centre_lines
            .get(&net)
            .is_some_and(|cells| cells.iter().any(|&i| self.occupancy[i] >= 2))
    }

    /// Ends a round: every cell where some net's centre line runs through
    /// another net's footprint gets `history_step` more history, and the
    /// present factor grows by `growth_percent`. Returns how many such
    /// collision cells there were — zero means the routes are compatible.
    pub fn end_round(&mut self, history_step: i64, growth_percent: i64) -> usize {
        let mut collisions = HashSet::new();
        for cells in self.centre_lines.values() {
            for &i in cells {
                // A net's own footprint always covers its centre line once.
                if self.occupancy[i] >= 2 {
                    collisions.insert(i);
                }
            }
        }
        for &i in &collisions {
            self.history[i] += history_step;
        }
        self.present_factor = self.present_factor * (100 + growth_percent) / 100;
        collisions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::Polygon;
    use tessera_model::Layer;

    fn board() -> Board {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
        board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
        board.outline = Some(Polygon::new(vec![
            Point::new(0, 0),
            Point::new(10_000_000, 0),
            Point::new(10_000_000, 10_000_000),
        ]));
        board
    }

    fn path(y: i64) -> RoutedPath {
        RoutedPath {
            segments: vec![(
                Segment::new(Point::new(1_000_000, y), Point::new(9_000_000, y)),
                LayerId(0),
            )],
            vias: vec![],
        }
    }

    #[test]
    fn parallel_routes_at_legal_spacing_dont_collide_but_crossing_ones_do() {
        let mut c = Congestion::new(&board(), 100).expect("outline");
        c.set_net(NetId(1), &[&path(5_000_000)], 400_000, 600_000);
        c.set_net(NetId(2), &[&path(5_500_000)], 400_000, 600_000);
        assert_eq!(c.end_round(10, 0), 0);
        c.set_net(NetId(2), &[&path(5_200_000)], 400_000, 600_000);
        assert!(c.end_round(10, 0) > 0);
        assert!(c.cost(Point::new(5_000_000, 5_200_000), 0) > 0);
        // Removing a net removes its footprint again.
        c.clear_net(NetId(1));
        c.clear_net(NetId(2));
        assert_eq!(c.end_round(10, 0), 0);
    }

    #[test]
    fn a_nets_own_edges_never_collide_with_each_other() {
        let mut c = Congestion::new(&board(), 100).expect("outline");
        c.set_net(
            NetId(1),
            &[&path(5_000_000), &path(5_100_000)],
            400_000,
            600_000,
        );
        assert_eq!(c.end_round(10, 0), 0);
    }
}
