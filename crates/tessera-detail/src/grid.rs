use tessera_geom::Point;
use tessera_model::{Board, LayerId, NetId};

use crate::obstacle::{Obstacle, ObstacleKind};

/// Grid cell size for the M2 baseline router: 0.1mm. Small enough that a
/// trivial board's tracks aren't visibly forced onto a coarse lattice,
/// coarse enough that a local per-connection search window stays a
/// tractable number of cells. Not tuned against a real corpus yet —
/// revisit once one exists (plan §9.1).
pub const CELL_NM: i64 = 100_000;

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
    #[must_use]
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    pub fn build(
        board: &Board,
        obstacles: &[Obstacle],
        routed_net: NetId,
        route_half_width_nm: i64,
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
            if obstacle.net == routed_net {
                continue;
            }
            let Some(layer_index) = layers.iter().position(|&l| l == obstacle.layer) else {
                continue; // obstacle on a layer this router instance doesn't cover
            };
            // Fail closed, not open: if clearance can't be resolved (a
            // malformed board — missing net class), block rather than risk
            // silently routing through something that would fail real DRC.
            let clearance_nm = board
                .resolved_clearance_nm(obstacle.net, routed_net)
                .unwrap_or(500_000);
            // Frozen obstacles get the same clearance treatment as movable
            // ones here — being locked affects rip-up eligibility (M5, not
            // yet built), not how close new copper may approach it.
            let _ = ObstacleKind::Frozen; // documents that both kinds reach this code path identically
            let required_nm = clearance_nm + route_half_width_nm;

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
