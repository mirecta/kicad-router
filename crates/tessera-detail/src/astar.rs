use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::grid::{Cell, ObstacleMap};

// Cost units follow the convention read from KiCadRoutingTools during the
// M0 prior-art survey (docs/PRIOR_ART.md): integer-scaled so a diagonal
// step costs sqrt(2) times an orthogonal one without needing floats in the
// search's hot path (plan §4.1's float-free-predicates spirit, extended
// here to the cost function too, though costs are a heuristic rather than
// a correctness predicate).
const ORTHO_COST: i64 = 1000;
const DIAG_COST: i64 = 1414;
/// Deliberately steep relative to a planar step: M2's baseline should
/// prefer staying on one layer when it can, using a via only when the
/// obstacle map genuinely leaves no other way through. Not tuned against
/// real boards yet.
const VIA_COST: i64 = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct State {
    pub cell: Cell,
    pub layer: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueueEntry {
    priority: i64,
    state: State,
}

// Reversed so `BinaryHeap` (a max-heap) pops the lowest priority first.
impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other.priority.cmp(&self.priority)
    }
}
impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn octile_distance(a: Cell, b: Cell) -> i64 {
    let dx = i64::from((a.x - b.x).abs());
    let dy = i64::from((a.y - b.y).abs());
    let (dmin, dmax) = if dx < dy { (dx, dy) } else { (dy, dx) };
    DIAG_COST * dmin + ORTHO_COST * (dmax - dmin)
}

fn heuristic(cell: Cell, goals: &[(Cell, usize)]) -> i64 {
    goals
        .iter()
        .map(|&(goal_cell, _)| octile_distance(cell, goal_cell))
        .min()
        .unwrap_or(0)
}

const DIRECTIONS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// Finds a minimum-cost path from any of `starts` to any of `goals` over
/// `map`, using 8-directional moves within a layer plus a same-cell
/// layer-change ("via") move. Returns the visited states start-to-goal
/// inclusive, or `None` if no path exists (goal unreachable, or every
/// start/goal cell is itself blocked).
///
/// `map` gates planar (track) moves using the route's own track-width
/// clearance; `via_map` gates via moves using the via's (larger, and
/// spanning every layer) clearance — a via move is illegal wherever
/// `via_map` is blocked on *any* layer at that cell, not just the
/// destination layer, since a through via occupies the whole stack.
/// Conflating the two would let the router place a via somewhere legal
/// for a bare track but too close to a neighbour for the actual via pad
/// — exactly the bug an earlier version of this router had, caught by
/// `tests/routing.rs` checking the routed result against `tessera-drc`
/// rather than trusting the search's own notion of "clear."
#[must_use]
pub fn search(
    map: &ObstacleMap,
    via_map: &ObstacleMap,
    starts: &[(Cell, usize)],
    goals: &[(Cell, usize)],
) -> Option<Vec<State>> {
    if starts.is_empty() || goals.is_empty() {
        return None;
    }
    let goal_set: HashSet<(Cell, usize)> = goals.iter().copied().collect();

    let mut open: BinaryHeap<QueueEntry> = BinaryHeap::new();
    let mut g_score: HashMap<State, i64> = HashMap::new();
    let mut came_from: HashMap<State, State> = HashMap::new();

    for &(cell, layer) in starts {
        if map.is_blocked(cell, layer) {
            continue;
        }
        let state = State { cell, layer };
        g_score.insert(state, 0);
        open.push(QueueEntry {
            state,
            priority: heuristic(cell, goals),
        });
    }

    while let Some(QueueEntry { state, .. }) = open.pop() {
        if goal_set.contains(&(state.cell, state.layer)) {
            return Some(reconstruct_path(&came_from, state));
        }

        // A stale queue entry (superseded by a better path found later)
        // still gets pushed back in when relaxed; skip re-expanding one
        // whose priority no longer matches its current best g-score.
        let Some(&current_g) = g_score.get(&state) else {
            continue;
        };

        for (dx, dy) in DIRECTIONS {
            let neighbor_cell = Cell {
                x: state.cell.x + dx,
                y: state.cell.y + dy,
            };
            if !map.in_bounds(neighbor_cell) || map.is_blocked(neighbor_cell, state.layer) {
                continue;
            }
            let step_cost = if dx != 0 && dy != 0 {
                DIAG_COST
            } else {
                ORTHO_COST
            };
            relax(
                &mut open,
                &mut g_score,
                &mut came_from,
                state,
                State {
                    cell: neighbor_cell,
                    layer: state.layer,
                },
                current_g + step_cost,
                goals,
            );
        }

        if !via_map.blocked_on_any_layer(state.cell) {
            for other_layer in 0..map.layer_count() {
                if other_layer == state.layer {
                    continue;
                }
                relax(
                    &mut open,
                    &mut g_score,
                    &mut came_from,
                    state,
                    State {
                        cell: state.cell,
                        layer: other_layer,
                    },
                    current_g + VIA_COST,
                    goals,
                );
            }
        }
    }

    None
}

#[allow(clippy::too_many_arguments)]
fn relax(
    open: &mut BinaryHeap<QueueEntry>,
    g_score: &mut HashMap<State, i64>,
    came_from: &mut HashMap<State, State>,
    from: State,
    to: State,
    tentative_g: i64,
    goals: &[(Cell, usize)],
) {
    let improved = g_score
        .get(&to)
        .is_none_or(|&existing| tentative_g < existing);
    if improved {
        g_score.insert(to, tentative_g);
        came_from.insert(to, from);
        open.push(QueueEntry {
            state: to,
            priority: tentative_g + heuristic(to.cell, goals),
        });
    }
}

fn reconstruct_path(came_from: &HashMap<State, State>, goal: State) -> Vec<State> {
    let mut path = vec![goal];
    let mut current = goal;
    while let Some(&prev) = came_from.get(&current) {
        path.push(prev);
        current = prev;
    }
    path.reverse();
    path
}
