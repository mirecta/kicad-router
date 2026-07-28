//! PathFinder negotiated congestion (McMurchie & Ebeling, FPGA 1995) over a
//! coarse global-routing grid — plan §5.2, and per plan §1 "the single
//! most important paper in the plan."
//!
//! This module is standalone: it decides which G-cells each net's coarse
//! route crosses so that, where physically possible, no G-cell edge is
//! asked to carry more tracks than it has room for. It does **not** yet
//! feed that decision into `tessera-detail`'s search as a corridor
//! constraint — today `tessera-detail` still searches a local window
//! around each connection's endpoints independently, unaware of global
//! congestion. Wiring the two together (so the detailed router is biased
//! toward, or confined to, the global router's chosen corridor) is
//! necessary follow-up work, tracked separately, not done here.
//!
//! Deliberately coarser than `tessera-detail::ObstacleMap`: this grid
//! tracks a flat per-layer edge capacity (how many tracks' worth of pitch
//! fit across one G-cell edge), not real obstacle geometry — a full
//! implementation would subtract each cell's actual fixed obstacles
//! (locked items, board edge, existing copper) from its available
//! capacity. That's a known simplification, appropriate for a first
//! implementation with no real corpus yet to measure the impact against.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use tessera_geom::Point;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GCell {
    pub x: i32,
    pub y: i32,
    pub layer: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EdgeKey(GCell, GCell);

impl EdgeKey {
    fn new(a: GCell, b: GCell) -> Self {
        if a <= b {
            EdgeKey(a, b)
        } else {
            EdgeKey(b, a)
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

/// The coarse global-routing grid: board-wide extent, uniform per-layer
/// edge capacity. See module docs for the capacity-model simplification.
#[derive(Debug, Clone)]
pub struct GlobalGrid {
    pub origin: Point,
    pub cell_size_nm: i64,
    pub width: i32,
    pub height: i32,
    /// Edge capacity per layer index — how many tracks' worth of pitch fit
    /// across one G-cell edge on that layer.
    pub layer_capacity: Vec<i64>,
}

impl GlobalGrid {
    #[must_use]
    pub fn cell_of(&self, point: Point, layer: usize) -> GCell {
        GCell {
            x: round_div_saturating(point.x - self.origin.x, self.cell_size_nm),
            y: round_div_saturating(point.y - self.origin.y, self.cell_size_nm),
            layer,
        }
    }

    fn in_bounds(&self, cell: GCell) -> bool {
        cell.x >= 0
            && cell.y >= 0
            && cell.x < self.width
            && cell.y < self.height
            && cell.layer < self.layer_capacity.len()
    }

    fn neighbors(&self, cell: GCell) -> Vec<GCell> {
        let mut result = Vec::new();
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let candidate = GCell {
                x: cell.x + dx,
                y: cell.y + dy,
                layer: cell.layer,
            };
            if self.in_bounds(candidate) {
                result.push(candidate);
            }
        }
        for layer in 0..self.layer_capacity.len() {
            if layer != cell.layer {
                result.push(GCell { layer, ..cell });
            }
        }
        result
    }

    fn capacity(&self, edge: EdgeKey) -> i64 {
        // A same-layer edge's capacity is that layer's; a layer-change
        // ("via") edge is capacity-unlimited here — via congestion isn't
        // modelled by this grid (a real implementation would give via
        // edges their own capacity derived from via pitch).
        if edge.0.layer == edge.1.layer {
            self.layer_capacity.get(edge.0.layer).copied().unwrap_or(0)
        } else {
            i64::MAX
        }
    }
}

/// A single net's global-routing request: candidate start/goal cells (a
/// pin usually maps to one cell, but may offer several if it's reachable
/// on multiple layers).
#[derive(Debug, Clone)]
pub struct NetRequest {
    pub starts: Vec<GCell>,
    pub goals: Vec<GCell>,
}

/// One net's routed path through the grid, in cell order.
pub type GlobalPath = Vec<GCell>;

/// Result of a negotiation run: the best path found per net (in request
/// order; `None` if that net was completely unreachable, which only
/// happens if its start/goal cells are out of bounds — congestion alone
/// never prevents *finding* a path, only makes it cost more), plus any
/// edges still over capacity when the run ended.
#[derive(Debug, Clone)]
pub struct NegotiationResult {
    pub paths: Vec<Option<GlobalPath>>,
    pub converged: bool,
    /// `(from, to, usage, capacity)` for every edge still over capacity
    /// when the run ended (empty iff `converged`).
    pub overused_edges: Vec<(GCell, GCell, i64, i64)>,
}

const BASE_EDGE_COST: i64 = 100;
const HISTORY_INCREMENT: i64 = 50;
const INITIAL_PRESENT_FACTOR: f64 = 1.0;
const PRESENT_FACTOR_GROWTH: f64 = 1.5;

/// Runs PathFinder negotiated congestion for up to `max_iterations` rounds
/// (plan §5.2's pseudocode, implemented directly): each round routes every
/// net in order via Dijkstra over a congestion-aware cost, using usage
/// that updates live as each net commits within the round (so net 5 in a
/// round already sees nets 1-4's usage, not just the previous round's);
/// after each round, edges left over capacity accumulate history cost
/// (which never decreases) and the present-congestion penalty sharpens,
/// pushing nets to negotiate away from contested edges over successive
/// rounds rather than oscillating. Stops early once no edge is over
/// capacity.
#[must_use]
pub fn negotiate(
    grid: &GlobalGrid,
    requests: &[NetRequest],
    max_iterations: usize,
) -> NegotiationResult {
    let mut history: HashMap<EdgeKey, i64> = HashMap::new();
    let mut present_factor = INITIAL_PRESENT_FACTOR;
    let mut last_paths: Vec<Option<GlobalPath>> = vec![None; requests.len()];
    let mut last_usage: HashMap<EdgeKey, i64> = HashMap::new();

    for _iteration in 0..max_iterations.max(1) {
        let mut usage: HashMap<EdgeKey, i64> = HashMap::new();
        let mut paths: Vec<Option<GlobalPath>> = Vec::with_capacity(requests.len());

        for request in requests {
            let path = shortest_path(grid, request, &usage, &history, present_factor);
            if let Some(path) = &path {
                for window in path.windows(2) {
                    let edge = EdgeKey::new(window[0], window[1]);
                    *usage.entry(edge).or_insert(0) += 1;
                }
            }
            paths.push(path);
        }

        let overused: Vec<(EdgeKey, i64, i64)> = usage
            .iter()
            .filter_map(|(&edge, &used)| {
                let capacity = grid.capacity(edge);
                (used > capacity).then_some((edge, used, capacity))
            })
            .collect();

        last_paths = paths;
        last_usage = usage;

        if overused.is_empty() {
            return NegotiationResult {
                paths: last_paths,
                converged: true,
                overused_edges: Vec::new(),
            };
        }

        for (edge, used, capacity) in &overused {
            *history.entry(*edge).or_insert(0) += HISTORY_INCREMENT * (used - capacity);
        }
        present_factor *= PRESENT_FACTOR_GROWTH;
    }

    let overused_edges: Vec<(GCell, GCell, i64, i64)> = last_usage
        .iter()
        .filter_map(|(&edge, &used)| {
            let capacity = grid.capacity(edge);
            (used > capacity).then_some((edge.0, edge.1, used, capacity))
        })
        .collect();

    NegotiationResult {
        paths: last_paths,
        converged: overused_edges.is_empty(),
        overused_edges,
    }
}

#[derive(PartialEq)]
struct HeapEntry {
    cost: f64,
    cell: GCell,
}

impl Eq for HeapEntry {}
impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(Ordering::Equal)
    }
}
impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn edge_cost(
    edge: EdgeKey,
    grid: &GlobalGrid,
    usage: &HashMap<EdgeKey, i64>,
    history: &HashMap<EdgeKey, i64>,
    present_factor: f64,
) -> f64 {
    let capacity = grid.capacity(edge);
    let used = usage.get(&edge).copied().unwrap_or(0);
    let hist = history.get(&edge).copied().unwrap_or(0);
    // "Would-be" usage if this net also crosses this edge — matches
    // PathFinder's own formulation of present congestion.
    let would_be = used + 1;
    let overuse = if capacity == i64::MAX {
        0
    } else {
        (would_be - capacity).max(0)
    };
    #[allow(clippy::cast_precision_loss)]
    let present = 1.0 + present_factor * (overuse as f64);
    #[allow(clippy::cast_precision_loss)]
    let base_plus_history = (BASE_EDGE_COST + hist) as f64;
    base_plus_history * present
}

fn shortest_path(
    grid: &GlobalGrid,
    request: &NetRequest,
    usage: &HashMap<EdgeKey, i64>,
    history: &HashMap<EdgeKey, i64>,
    present_factor: f64,
) -> Option<GlobalPath> {
    let goal_set: std::collections::HashSet<GCell> = request.goals.iter().copied().collect();
    let mut dist: HashMap<GCell, f64> = HashMap::new();
    let mut came_from: HashMap<GCell, GCell> = HashMap::new();
    let mut heap = BinaryHeap::new();

    for &start in &request.starts {
        if grid.in_bounds(start) {
            dist.insert(start, 0.0);
            heap.push(HeapEntry {
                cost: 0.0,
                cell: start,
            });
        }
    }

    while let Some(HeapEntry { cost, cell }) = heap.pop() {
        if goal_set.contains(&cell) {
            return Some(reconstruct(&came_from, cell));
        }
        if cost > *dist.get(&cell).unwrap_or(&f64::INFINITY) {
            continue;
        }
        for neighbor in grid.neighbors(cell) {
            let edge = EdgeKey::new(cell, neighbor);
            let new_cost = cost + edge_cost(edge, grid, usage, history, present_factor);
            if new_cost < *dist.get(&neighbor).unwrap_or(&f64::INFINITY) {
                dist.insert(neighbor, new_cost);
                came_from.insert(neighbor, cell);
                heap.push(HeapEntry {
                    cost: new_cost,
                    cell: neighbor,
                });
            }
        }
    }

    None
}

fn reconstruct(came_from: &HashMap<GCell, GCell>, goal: GCell) -> GlobalPath {
    let mut path = vec![goal];
    let mut current = goal;
    while let Some(&prev) = came_from.get(&current) {
        path.push(prev);
        current = prev;
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_grid(width: i32, height: i32, capacity: i64) -> GlobalGrid {
        GlobalGrid {
            origin: Point::new(0, 0),
            cell_size_nm: 500_000,
            width,
            height,
            layer_capacity: vec![capacity],
        }
    }

    fn cell(x: i32, y: i32) -> GCell {
        GCell { x, y, layer: 0 }
    }

    #[test]
    fn single_net_routes_directly_with_no_congestion() {
        let grid = small_grid(10, 10, 4);
        let request = NetRequest {
            starts: vec![cell(0, 0)],
            goals: vec![cell(5, 0)],
        };
        let result = negotiate(&grid, &[request], 10);
        assert!(result.converged);
        assert!(result.paths[0].is_some());
    }

    #[test]
    fn two_nets_share_capacity_when_it_fits() {
        // A 1-wide corridor with capacity 2: both nets can use it at once.
        let grid = small_grid(10, 3, 2);
        let requests = vec![
            NetRequest {
                starts: vec![cell(0, 1)],
                goals: vec![cell(5, 1)],
            },
            NetRequest {
                starts: vec![cell(0, 1)],
                goals: vec![cell(5, 1)],
            },
        ];
        let result = negotiate(&grid, &requests, 20);
        assert!(result.converged, "{:?}", result.overused_edges);
    }

    #[test]
    fn congestion_pushes_nets_onto_an_alternate_route_when_one_exists() {
        // A grid with two parallel horizontal corridors (y=0 and y=2) of
        // capacity 1 each, connected by a shared start/goal region. Two
        // nets both wanting to go left-to-right must end up on different
        // rows once negotiated, since neither row alone has capacity 2.
        let grid = small_grid(10, 3, 1);
        let requests = vec![
            NetRequest {
                starts: vec![cell(0, 0), cell(0, 2)],
                goals: vec![cell(9, 0), cell(9, 2)],
            },
            NetRequest {
                starts: vec![cell(0, 0), cell(0, 2)],
                goals: vec![cell(9, 0), cell(9, 2)],
            },
        ];
        let result = negotiate(&grid, &requests, 30);
        assert!(result.converged, "{:?}", result.overused_edges);

        // The two nets must not have collapsed onto the same row for
        // their entire path — otherwise capacity 1 would be violated,
        // which `converged` already rules out, but this double-checks the
        // *mechanism* actually diversified them rather than than one net
        // just failing to route.
        let path0 = result.paths[0].as_ref().unwrap();
        let path1 = result.paths[1].as_ref().unwrap();
        assert_ne!(
            path0.first().unwrap().y,
            path1.first().unwrap().y,
            "negotiation should have routed the two nets on different rows"
        );
    }

    #[test]
    fn genuinely_infeasible_congestion_is_reported_not_hidden() {
        // Three nets, only two units of capacity anywhere: cannot all
        // converge. Must report remaining overuse honestly rather than
        // claim success.
        let grid = small_grid(5, 1, 2);
        let requests = vec![
            NetRequest {
                starts: vec![cell(0, 0)],
                goals: vec![cell(4, 0)],
            },
            NetRequest {
                starts: vec![cell(0, 0)],
                goals: vec![cell(4, 0)],
            },
            NetRequest {
                starts: vec![cell(0, 0)],
                goals: vec![cell(4, 0)],
            },
        ];
        let result = negotiate(&grid, &requests, 15);
        assert!(!result.converged);
        assert!(!result.overused_edges.is_empty());
        // All three nets should still have *some* path reported (routing
        // doesn't just give up), even though it's over capacity.
        assert!(result.paths.iter().all(Option::is_some));
    }
}
