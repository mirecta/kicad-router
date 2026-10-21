use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::grid::{Cell, CorridorMask, ObstacleMap};

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
    /// Index into `DIRECTIONS` of the step that arrived here, or
    /// `NO_DIRECTION` at a start or right after a via.
    pub dir: u8,
}

const NO_DIRECTION: u8 = 8;

/// Extra cost of a diagonal step on top of `DIAG_COST`, making it just
/// dearer than the two straight steps it replaces (2114 vs 2000): routes
/// run mostly horizontally and vertically with 45° runs where they pay off,
/// rather than long diagonals across the board. Tuned on a real board
/// (1000 → too few diagonals for the user's taste; 500 or less → long
/// diagonals cut across other nets' paths and four more nets fail).
const DIAG_EXTRA: i64 = 700;
/// Extra cost of a 45° turn and of a 90° turn, so a route keeps going
/// straight rather than zig-zagging. Sharper turns aren't allowed at all.
const BEND_45: i64 = 100;
const BEND_90: i64 = 300;

/// The direction's angle in 45° steps (0 = +x, counting towards +y).
fn angle_index(dir: usize) -> i32 {
    match DIRECTIONS[dir] {
        (1, 0) => 0,
        (1, 1) => 1,
        (0, 1) => 2,
        (-1, 1) => 3,
        (-1, 0) => 4,
        (-1, -1) => 5,
        (0, -1) => 6,
        _ => 7,
    }
}

/// Extra cost of turning from `from` into `to`, or `None` for a turn
/// sharper than 90° (an acute track corner — never wanted).
fn bend_cost(from: u8, to: usize) -> Option<i64> {
    if from == NO_DIRECTION {
        return Some(0);
    }
    let d = (angle_index(usize::from(from)) - angle_index(to)).rem_euclid(8);
    match d.min(8 - d) {
        0 => Some(0),
        1 => Some(BEND_45),
        2 => Some(BEND_90),
        _ => None,
    }
}

fn step_cost(dx: i32, dy: i32) -> i64 {
    if dx != 0 && dy != 0 {
        DIAG_COST + DIAG_EXTRA
    } else {
        ORTHO_COST
    }
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

/// The cheapest possible cost from `a` to `b` ignoring obstacles and
/// bends — an admissible A* heuristic. A diagonal step is counted at
/// whichever is cheaper, itself or two straight steps, so the estimate
/// stays tight now that diagonals carry `DIAG_EXTRA`.
fn octile_distance(a: Cell, b: Cell) -> i64 {
    let dx = i64::from((a.x - b.x).abs());
    let dy = i64::from((a.y - b.y).abs());
    let (dmin, dmax) = if dx < dy { (dx, dy) } else { (dy, dx) };
    let diagonal = (DIAG_COST + DIAG_EXTRA).min(2 * ORTHO_COST);
    diagonal * dmin + ORTHO_COST * (dmax - dmin)
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
///
/// `corridor`, if given, additionally forbids expanding into any cell it
/// doesn't cover — the hard corridor constraint `route_connection` applies
/// when the global router supplied waypoints. `None` searches the full
/// window, same as before that constraint existed.
#[must_use]
pub fn search(
    map: &ObstacleMap,
    via_map: &ObstacleMap,
    starts: &[(Cell, usize)],
    goals: &[(Cell, usize)],
    corridor: Option<&CorridorMask>,
) -> Option<Vec<State>> {
    if starts.is_empty() || goals.is_empty() {
        return None;
    }
    let goal_set: HashSet<(Cell, usize)> = goals.iter().copied().collect();
    // Dense per-state bookkeeping over the search window (cell × layer ×
    // incoming direction) — a `HashMap` here was most of the router's run
    // time on a real board.
    let space = StateSpace::new(map);
    let mut g_score = vec![i64::MAX; space.len()];
    let mut came_from = vec![usize::MAX; space.len()];
    let mut open: BinaryHeap<QueueEntry> = BinaryHeap::new();

    for &(cell, layer) in starts {
        if map.is_blocked(cell, layer) {
            continue;
        }
        let state = State {
            cell,
            layer,
            dir: NO_DIRECTION,
        };
        let Some(i) = space.index(state) else {
            continue;
        };
        g_score[i] = 0;
        open.push(QueueEntry {
            state,
            priority: heuristic(cell, goals),
        });
    }

    while let Some(QueueEntry { state, priority }) = open.pop() {
        let Some(current) = space.index(state) else {
            continue;
        };
        let current_g = g_score[current];
        if priority > current_g + heuristic(state.cell, goals) {
            continue; // stale: a cheaper way here was found after this was queued
        }
        if goal_set.contains(&(state.cell, state.layer)) {
            return Some(space.path(&came_from, current));
        }

        let mut relax = |to: State, g: i64| {
            if let Some(i) = space.index(to) {
                if g < g_score[i] {
                    g_score[i] = g;
                    came_from[i] = current;
                    open.push(QueueEntry {
                        state: to,
                        priority: g + heuristic(to.cell, goals),
                    });
                }
            }
        };

        for (dir, (dx, dy)) in DIRECTIONS.into_iter().enumerate() {
            let neighbor_cell = Cell {
                x: state.cell.x + dx,
                y: state.cell.y + dy,
            };
            if !map.in_bounds(neighbor_cell) || map.is_blocked(neighbor_cell, state.layer) {
                continue;
            }
            if corridor.is_some_and(|c| !c.contains(neighbor_cell)) {
                continue;
            }
            let Some(bend) = bend_cost(state.dir, dir) else {
                continue;
            };
            relax(
                State {
                    cell: neighbor_cell,
                    layer: state.layer,
                    dir: u8::try_from(dir).unwrap_or(NO_DIRECTION),
                },
                current_g + step_cost(dx, dy) + bend,
            );
        }

        if !via_map.blocked_on_any_layer(state.cell) {
            for other_layer in 0..map.layer_count() {
                if other_layer != state.layer {
                    relax(
                        State {
                            cell: state.cell,
                            layer: other_layer,
                            dir: NO_DIRECTION,
                        },
                        current_g + VIA_COST,
                    );
                }
            }
        }
    }

    None
}

/// Maps every search state of one window to a dense index.
struct StateSpace {
    width: usize,
    height: usize,
    layers: usize,
}

impl StateSpace {
    const DIRS: usize = NO_DIRECTION as usize + 1;

    fn new(map: &ObstacleMap) -> Self {
        let (w, h) = map.size();
        Self {
            width: usize::try_from(w).unwrap_or(0),
            height: usize::try_from(h).unwrap_or(0),
            layers: map.layer_count(),
        }
    }

    fn len(&self) -> usize {
        self.width * self.height * self.layers * Self::DIRS
    }

    fn index(&self, s: State) -> Option<usize> {
        let x = usize::try_from(s.cell.x).ok().filter(|&x| x < self.width)?;
        let y = usize::try_from(s.cell.y)
            .ok()
            .filter(|&y| y < self.height)?;
        Some(((y * self.width + x) * self.layers + s.layer) * Self::DIRS + usize::from(s.dir))
    }

    fn state(&self, i: usize) -> State {
        let dir = i % Self::DIRS;
        let rest = i / Self::DIRS;
        let layer = rest % self.layers;
        let cell = rest / self.layers;
        State {
            cell: Cell {
                x: i32::try_from(cell % self.width).unwrap_or(0),
                y: i32::try_from(cell / self.width).unwrap_or(0),
            },
            layer,
            dir: u8::try_from(dir).unwrap_or(NO_DIRECTION),
        }
    }

    fn path(&self, came_from: &[usize], goal: usize) -> Vec<State> {
        let mut path = vec![self.state(goal)];
        let mut current = goal;
        while let Some(&prev) = came_from.get(current).filter(|&&p| p != usize::MAX) {
            path.push(self.state(prev));
            current = prev;
        }
        path.reverse();
        path
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

/// The cheapest same-layer path from `start` to any cell `accept` admits,
/// exploring at most `max_cost` (in step-cost units) — a Dijkstra search,
/// since there's no single goal to aim a heuristic at. The start cell
/// itself is never accepted.
#[must_use]
pub fn nearest(
    map: &ObstacleMap,
    start: (Cell, usize),
    max_cost: i64,
    accept: impl Fn(Cell) -> bool,
) -> Option<Vec<State>> {
    let (start_cell, layer) = start;
    if map.is_blocked(start_cell, layer) {
        return None;
    }
    let start_state = State {
        cell: start_cell,
        layer,
        dir: NO_DIRECTION,
    };
    let mut open = BinaryHeap::new();
    let mut g_score: HashMap<State, i64> = HashMap::new();
    let mut came_from: HashMap<State, State> = HashMap::new();
    g_score.insert(start_state, 0);
    open.push(QueueEntry {
        priority: 0,
        state: start_state,
    });
    while let Some(QueueEntry { priority, state }) = open.pop() {
        if g_score.get(&state).is_some_and(|&g| g < priority) {
            continue; // superseded
        }
        if state.cell != start_cell && accept(state.cell) {
            return Some(reconstruct_path(&came_from, state));
        }
        for (dx, dy) in DIRECTIONS {
            let cell = Cell {
                x: state.cell.x + dx,
                y: state.cell.y + dy,
            };
            if !map.in_bounds(cell) || map.is_blocked(cell, layer) {
                continue;
            }
            let step = if dx != 0 && dy != 0 {
                DIAG_COST
            } else {
                ORTHO_COST
            };
            let g = priority + step;
            if g > max_cost {
                continue;
            }
            let next = State {
                cell,
                layer,
                dir: NO_DIRECTION,
            };
            if g_score.get(&next).is_none_or(|&existing| g < existing) {
                g_score.insert(next, g);
                came_from.insert(next, state);
                open.push(QueueEntry {
                    priority: g,
                    state: next,
                });
            }
        }
    }
    None
}
