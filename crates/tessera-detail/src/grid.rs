use tessera_geom::{Circle, Point, Segment};
use tessera_model::{Board, LayerId, NetId};

use crate::obstacle::{Obstacle, ObstacleKind, ObstacleShape};

/// Grid cell size for the M2 baseline router: 0.1mm. Small enough that a
/// trivial board's tracks aren't visibly forced onto a coarse lattice,
/// coarse enough that a local per-connection search window stays a
/// tractable number of cells. Not tuned against a real corpus yet —
/// revisit once one exists (plan §9.1).
pub const CELL_NM: i64 = 100_000;

/// Squared half-length of a diagonal grid move — the longest straight move
/// the 8-directional search makes (`(CELL_NM * sqrt(2) / 2)^2`, exact).
pub const MOVE_HALF_LENGTH_SQ_NM: i64 = CELL_NM * CELL_NM / 2;

/// The smallest integer `r'` with `r'^2 >= r^2 + half_length_sq` — see
/// [`ObstacleMap::build`] for why that radius makes every move safe.
#[allow(clippy::cast_possible_truncation)]
fn move_safe_radius_nm(radius_nm: i64, half_length_sq_nm: i64) -> i64 {
    let target = i128::from(radius_nm) * i128::from(radius_nm) + i128::from(half_length_sq_nm);
    let root = target.isqrt();
    let ceil = if root * root < target { root + 1 } else { root };
    ceil as i64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cell {
    pub x: i32,
    pub y: i32,
}

impl Cell {
    #[must_use]
    pub fn to_point(self, origin: Point) -> Point {
        Point::new(
            origin.x + i64::from(self.x) * CELL_NM,
            origin.y + i64::from(self.y) * CELL_NM,
        )
    }

    /// Snaps `point` to the nearest cell relative to `origin`, saturating
    /// rather than panicking if a wildly out-of-range point is ever passed
    /// in (shouldn't happen given `MAX_COORDINATE_NM`, but this is cheap
    /// insurance against a silent wraparound bug instead).
    #[must_use]
    pub fn from_point(point: Point, origin: Point) -> Self {
        Self {
            x: round_div_saturating(point.x - origin.x, CELL_NM),
            y: round_div_saturating(point.y - origin.y, CELL_NM),
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn round_div_saturating(numerator: i64, denominator: i64) -> i32 {
    let half = denominator / 2;
    let rounded = if numerator >= 0 {
        (numerator + half) / denominator
    } else {
        -((-numerator + half) / denominator)
    };
    rounded.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// The local search window an [`ObstacleMap`] (and the A* search over it)
/// operates within: `origin` is the window's lower-left corner in board
/// coordinates, `width`/`height` its extent in cells.
#[derive(Debug, Clone, Copy)]
pub struct GridBounds {
    pub origin: Point,
    pub width: i32,
    pub height: i32,
}

/// A precomputed per-layer bitmap of which grid cells a new track for
/// `routed_net` may not enter, at a given required half-width.
///
/// Obstacles belonging to `routed_net` itself never block (its own pads are
/// the route's start/end, not obstacles to avoid). Cells are blocked using
/// each obstacle's exact `tessera-geom` clearance predicate evaluated at
/// the cell's centre point — this is deliberately the same exact-geometry
/// primitive `tessera-drc` uses, not a separate approximation, so the
/// router's notion of "clear" agrees with the DRC engine's by construction
/// rather than by coincidence.
pub struct ObstacleMap {
    origin: Point,
    width: i32,
    height: i32,
    layer_count: usize,
    blocked: Vec<bool>,
}

impl ObstacleMap {
    // Sign loss is safe: the bounds check above guarantees cell.x/cell.y
    // are non-negative before this cast ever runs.
    #[allow(clippy::cast_sign_loss)]
    fn index(&self, cell: Cell, layer_index: usize) -> Option<usize> {
        if cell.x < 0 || cell.y < 0 || cell.x >= self.width || cell.y >= self.height {
            return None;
        }
        let planar = (cell.y as usize) * (self.width as usize) + (cell.x as usize);
        Some(planar * self.layer_count + layer_index)
    }

    #[must_use]
    pub fn is_blocked(&self, cell: Cell, layer_index: usize) -> bool {
        self.index(cell, layer_index)
            .is_none_or(|i| self.blocked[i])
    }

    #[must_use]
    pub fn in_bounds(&self, cell: Cell) -> bool {
        cell.x >= 0 && cell.y >= 0 && cell.x < self.width && cell.y < self.height
    }

    #[must_use]
    pub fn layer_count(&self) -> usize {
        self.layer_count
    }

    /// True iff `cell` is blocked on *any* layer this map covers — the
    /// right check for via legality, since a through via (M2's only via
    /// kind) occupies every layer it spans, not just the one layer a
    /// track's clearance check cares about.
    #[must_use]
    pub fn blocked_on_any_layer(&self, cell: Cell) -> bool {
        (0..self.layer_count).any(|layer| self.is_blocked(cell, layer))
    }

    #[must_use]
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// Builds the blocked-cell bitmap for a route on `routed_net` with
    /// `route_half_width_nm` (half the new track's width — approaching an
    /// obstacle any closer than `clearance + route_half_width_nm` would
    /// violate DRC). `layers` gives the layer index each `LayerId` maps to
    /// (M2 scope: exactly `F.Cu` and `B.Cu`, indices 0 and 1).
    ///
    /// `move_half_length_sq_nm` is the squared half-length of the longest
    /// straight move the search makes between two unblocked cell centres
    /// ([`MOVE_HALF_LENGTH_SQ_NM`] for 8-directional track moves, `0` for a
    /// map only ever queried at single points, like via placement). Only
    /// checking cell centres would let a move between two clear centres
    /// graze an obstacle mid-way. Blocking at `R' = ceil(sqrt(R² + h²))`
    /// instead of `R` closes that gap exactly, for any obstacle shape: the
    /// obstacle point nearest the move is at least `R'` from both of its
    /// ends, so at least `sqrt(R'² - h²) >= R` from every point on it.
    #[must_use]
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    pub fn build(
        board: &Board,
        obstacles: &[Obstacle],
        routed_net: NetId,
        route_half_width_nm: i64,
        move_half_length_sq_nm: i64,
        bounds: GridBounds,
        layers: &[LayerId],
    ) -> Self {
        let GridBounds {
            origin,
            width,
            height,
        } = bounds;
        let layer_count = layers.len();
        let mut blocked = vec![false; (width as usize) * (height as usize) * layer_count];

        for obstacle in obstacles {
            if obstacle.net.same_net(routed_net) {
                continue;
            }
            let Some(layer_index) = layers.iter().position(|&l| l == obstacle.layer) else {
                continue; // obstacle on a layer this router instance doesn't cover
            };
            // Fail closed, not open: if clearance can't be resolved (a
            // malformed board — missing net class), block rather than risk
            // silently routing through something that would fail real DRC.
            let clearance_nm = obstacle
                .fixed_clearance_nm
                .or_else(|| board.resolved_clearance_nm(obstacle.net, routed_net))
                .unwrap_or(500_000);
            // Frozen obstacles get the same clearance treatment as movable
            // ones here — being locked affects rip-up eligibility (M5, not
            // yet built), not how close new copper may approach it.
            let _ = ObstacleKind::Frozen; // documents that both kinds reach this code path identically
            let required_nm =
                move_safe_radius_nm(clearance_nm + route_half_width_nm, move_half_length_sq_nm);

            let (min_pt, max_pt) = obstacle.shape.bounding_box(required_nm);
            let min_cell = Cell::from_point(min_pt, origin);
            let max_cell = Cell::from_point(max_pt, origin);

            for cy in min_cell.y.max(0)..=max_cell.y.min(height - 1) {
                for cx in min_cell.x.max(0)..=max_cell.x.min(width - 1) {
                    let cell = Cell { x: cx, y: cy };
                    let point = cell.to_point(origin);
                    if !obstacle.shape.clears_point(point, required_nm) {
                        let planar = (cy as usize) * (width as usize) + (cx as usize);
                        blocked[planar * layer_count + layer_index] = true;
                    }
                }
            }
        }

        Self {
            origin,
            width,
            height,
            layer_count,
            blocked,
        }
    }
}

impl ObstacleMap {
    /// Blocks, on every layer, each cell where a new via's drill (radius
    /// `drill_radius_nm`) would come within `min_hole_to_hole_nm` of an
    /// existing drill. Unlike copper clearance this ignores nets — verified
    /// against real `kicad-cli`: `hole_to_hole` applies between two vias of
    /// the same net, and between a via and a same-net plated pad.
    #[allow(clippy::cast_sign_loss)]
    pub fn block_drill_conflicts(
        &mut self,
        drills: &[Circle],
        drill_radius_nm: i64,
        min_hole_to_hole_nm: i64,
    ) {
        for drill in drills {
            let reach = drill.radius_nm + drill_radius_nm + min_hole_to_hole_nm;
            let min_cell = Cell::from_point(
                Point::new(drill.center.x - reach, drill.center.y - reach),
                self.origin,
            );
            let max_cell = Cell::from_point(
                Point::new(drill.center.x + reach, drill.center.y + reach),
                self.origin,
            );
            for cy in min_cell.y.max(0)..=max_cell.y.min(self.height - 1) {
                for cx in min_cell.x.max(0)..=max_cell.x.min(self.width - 1) {
                    let cell = Cell { x: cx, y: cy };
                    let via_drill = Circle::new(cell.to_point(self.origin), drill_radius_nm);
                    if !drill.clears_circle(via_drill, min_hole_to_hole_nm) {
                        for layer in 0..self.layer_count {
                            if let Some(i) = self.index(cell, layer) {
                                self.blocked[i] = true;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A precomputed per-cell mask of which cells lie within `half_width_nm` of
/// the polyline connecting a connection's endpoints through the global
/// router's waypoint hints — the "hard" tube a corridor-constrained search
/// confines itself to, tighter than the bounding-box-only widening
/// `route_connection` used to do on its own. Layer-independent: the coarse
/// global path doesn't pick a physical layer, so the same tube applies on
/// every layer of the fine grid.
pub struct CorridorMask {
    width: i32,
    height: i32,
    inside: Vec<bool>,
}

impl CorridorMask {
    /// Builds the mask over `bounds`'s cells from `polyline` (at least 2
    /// points — a degenerate single-point polyline marks nothing inside
    /// and is a caller bug, not handled specially here).
    #[must_use]
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    pub fn build(polyline: &[Point], half_width_nm: i64, bounds: GridBounds) -> Self {
        let GridBounds {
            origin,
            width,
            height,
        } = bounds;
        let mut inside = vec![false; (width as usize) * (height as usize)];

        for window in polyline.windows(2) {
            let shape = ObstacleShape::Segment(Segment::new(window[0], window[1]), 0);
            let (min_pt, max_pt) = shape.bounding_box(half_width_nm);
            let min_cell = Cell::from_point(min_pt, origin);
            let max_cell = Cell::from_point(max_pt, origin);

            for cy in min_cell.y.max(0)..=max_cell.y.min(height - 1) {
                for cx in min_cell.x.max(0)..=max_cell.x.min(width - 1) {
                    let cell = Cell { x: cx, y: cy };
                    let point = cell.to_point(origin);
                    if !shape.clears_point(point, half_width_nm) {
                        let idx = (cy as usize) * (width as usize) + (cx as usize);
                        inside[idx] = true;
                    }
                }
            }
        }

        Self {
            width,
            height,
            inside,
        }
    }

    /// Forces `cell` inside the corridor regardless of its distance from
    /// the polyline — used to guarantee a connection's own start/goal
    /// cells are never excluded by grid-quantization rounding at the
    /// corridor's edge.
    pub fn mark_inside(&mut self, cell: Cell) {
        if let Some(idx) = self.index(cell) {
            self.inside[idx] = true;
        }
    }

    #[must_use]
    pub fn contains(&self, cell: Cell) -> bool {
        self.index(cell).is_some_and(|i| self.inside[i])
    }

    #[allow(clippy::cast_sign_loss)]
    fn index(&self, cell: Cell) -> Option<usize> {
        if cell.x < 0 || cell.y < 0 || cell.x >= self.width || cell.y >= self.height {
            return None;
        }
        Some((cell.y as usize) * (self.width as usize) + (cell.x as usize))
    }
}

#[cfg(test)]
mod corridor_tests {
    use super::*;

    #[test]
    fn contains_cells_near_the_polyline_and_excludes_cells_far_from_it() {
        let bounds = GridBounds {
            origin: Point::new(-1_000_000, -1_000_000),
            width: 40,
            height: 40,
        };
        let polyline = [Point::new(0, 0), Point::new(2_000_000, 0)];
        let corridor = CorridorMask::build(&polyline, 500_000, bounds);

        let on_the_line = Cell::from_point(Point::new(1_000_000, 0), bounds.origin);
        assert!(corridor.contains(on_the_line));

        let far_away = Cell::from_point(Point::new(1_000_000, 1_500_000), bounds.origin);
        assert!(!corridor.contains(far_away));

        assert!(!corridor.contains(Cell { x: -1, y: 0 }));
    }

    #[test]
    fn mark_inside_forces_a_cell_regardless_of_distance() {
        let bounds = GridBounds {
            origin: Point::new(-1_000_000, -1_000_000),
            width: 40,
            height: 40,
        };
        let polyline = [Point::new(0, 0), Point::new(2_000_000, 0)];
        let mut corridor = CorridorMask::build(&polyline, 500_000, bounds);

        let far_away = Cell::from_point(Point::new(1_000_000, 1_500_000), bounds.origin);
        assert!(!corridor.contains(far_away));

        corridor.mark_inside(far_away);
        assert!(corridor.contains(far_away));
    }
}

#[cfg(test)]
mod obstacle_map_tests {
    use tessera_geom::{Circle, Rect};
    use tessera_model::{Layer, Net, NetClass};

    use super::*;

    const CLEARANCE_NM: i64 = 200_000;
    const HALF_WIDTH_NM: i64 = 100_000;

    fn two_net_board() -> Board {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
        let mut class = NetClass::default_placeholder();
        class.clearance_nm = CLEARANCE_NM;
        board.net_classes.insert("Default".to_string(), class);
        for id in [1, 2] {
            board.nets.insert(
                NetId(id),
                Net {
                    id: NetId(id),
                    name: format!("N{id}"),
                    net_class: "Default".to_string(),
                },
            );
        }
        board
    }

    /// Every straight move between two unblocked neighbouring cells, for
    /// which `clears` (the obstacle's exact segment predicate) is false.
    fn unsafe_moves(
        shape: ObstacleShape,
        move_half_length_sq_nm: i64,
        clears: impl Fn(Segment) -> bool,
    ) -> usize {
        let board = two_net_board();
        let obstacle = Obstacle {
            shape,
            layer: LayerId(0),
            net: NetId(2),
            kind: ObstacleKind::Movable,
            fixed_clearance_nm: None,
        };
        let bounds = GridBounds {
            origin: Point::new(-2_000_000, -2_000_000),
            width: 40,
            height: 40,
        };
        let map = ObstacleMap::build(
            &board,
            &[obstacle],
            NetId(1),
            HALF_WIDTH_NM,
            move_half_length_sq_nm,
            bounds,
            &[LayerId(0)],
        );
        let mut count = 0;
        for y in 0..bounds.height {
            for x in 0..bounds.width {
                let from = Cell { x, y };
                if map.is_blocked(from, 0) {
                    continue;
                }
                for (dx, dy) in [(1, 0), (0, 1), (1, 1), (1, -1)] {
                    let to = Cell {
                        x: x + dx,
                        y: y + dy,
                    };
                    if !map.in_bounds(to) || map.is_blocked(to, 0) {
                        continue;
                    }
                    let seg =
                        Segment::new(from.to_point(bounds.origin), to.to_point(bounds.origin));
                    if !clears(seg) {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    fn sweep(move_half_length_sq_nm: i64) -> usize {
        let required = CLEARANCE_NM + HALF_WIDTH_NM;
        let mut total = 0;
        for ox in (0..CELL_NM).step_by(9_973) {
            for oy in (0..CELL_NM).step_by(9_973) {
                let center = Point::new(ox, oy);
                let rect = Rect::new(center, 1_000_000, 600_000);
                total += unsafe_moves(ObstacleShape::Rect(rect), move_half_length_sq_nm, |s| {
                    rect.clears_segment(s, required)
                });
                let circle = Circle::new(center, 400_000);
                total += unsafe_moves(ObstacleShape::Circle(circle), move_half_length_sq_nm, |s| {
                    circle.clears_segment(s, required)
                });
            }
        }
        total
    }

    #[test]
    fn checking_only_cell_centres_lets_moves_graze_obstacles() {
        assert!(sweep(0) > 0);
    }

    #[test]
    fn every_move_between_unblocked_cells_clears_every_obstacle() {
        assert_eq!(sweep(MOVE_HALF_LENGTH_SQ_NM), 0);
    }

    #[test]
    fn drill_conflicts_block_a_via_near_any_existing_drill() {
        let board = two_net_board();
        let bounds = GridBounds {
            origin: Point::new(0, 0),
            width: 20,
            height: 1,
        };
        let mut map = ObstacleMap::build(&board, &[], NetId(1), 0, 0, bounds, &[LayerId(0)]);
        // Existing drill radius 150_000 at x=0; new drill radius 150_000;
        // min 250_000 edge to edge => centres must be >= 550_000 apart.
        map.block_drill_conflicts(&[Circle::new(Point::new(0, 0), 150_000)], 150_000, 250_000);
        assert!(map.is_blocked(Cell { x: 5, y: 0 }, 0)); // 500_000
        assert!(!map.is_blocked(Cell { x: 6, y: 0 }, 0)); // 600_000
    }

    #[test]
    fn move_safe_radius_is_the_exact_integer_ceiling() {
        assert_eq!(move_safe_radius_nm(300_000, 0), 300_000);
        // 300_000^2 + 5_000_000_000 = 95_000_000_000; sqrt ~= 308_220.7
        assert_eq!(
            move_safe_radius_nm(300_000, MOVE_HALF_LENGTH_SQ_NM),
            308_221
        );
        assert_eq!(move_safe_radius_nm(3, 16), 5);
    }
}
