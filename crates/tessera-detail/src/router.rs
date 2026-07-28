use tessera_geom::{orient, Orientation, Point, Segment};
use tessera_model::{Board, Connection, LayerId};

use crate::astar::{self, State};
use crate::grid::{Cell, GridBounds, ObstacleMap, CELL_NM};
use crate::obstacle::obstacles_from_board;

/// A margin, in nanometres, added around a connection's endpoints when
/// sizing the local search grid — enough room to route around a nearby
/// obstacle without ballooning the search space for a short connection.
/// Not tuned against a real corpus yet; if a connection fails to route
/// because the true detour needs more room than this, that's a known
/// current limitation (a full-board grid, or an expanding-window retry,
/// are both natural follow-ups once a corpus exists to measure against).
const SEARCH_MARGIN_NM: i64 = 3_000_000;

/// One layer segment of a routed path, plus any via placed at its end to
/// change layers. `tessera-engine` (M2's orchestration layer) turns this
/// into actual `Track`/`Via` items to commit back to the board.
#[derive(Debug, Clone)]
pub struct RoutedPath {
    /// Straight segments in path order, each tagged with the layer it's on.
    pub segments: Vec<(Segment, LayerId)>,
    /// Via positions where the path changes layers, in path order.
    pub vias: Vec<Point>,
}

/// Routes `connection` on `board` using a grid octilinear A* search over a
/// local window (plan §5.3's grid baseline). Returns `None` if no path
/// exists within that window — this is the simple, expected-to-be-
/// imperfect M2/M3 baseline, not the eventual topological router; a `None`
/// here doesn't necessarily mean the connection is unroutable, only that
/// this baseline couldn't find a way through the local search window.
///
/// `waypoints` is an optional hint from the global router
/// (`tessera_global::pathfinder`'s negotiated path, converted to real
/// coordinates by the caller): the search window's bounding box expands to
/// cover every waypoint, not just `connection`'s own endpoints, so the
/// window follows the global router's chosen corridor instead of always
/// being a straight line between start and goal. This is a **soft**
/// influence — it reshapes where this function looks, not a hard
/// constraint forcing the path through those cells — because that's as
/// far as the integration goes today; a real corridor constraint (reject
/// cells outside a tube around the waypoints, not just widen the box)
/// would find shorter, more predictable paths, and is a natural next step,
/// not attempted here. Pass an empty slice for the old start/end-only
/// window.
#[must_use]
pub fn route_connection(
    board: &Board,
    connection: &Connection,
    waypoints: &[Point],
) -> Option<RoutedPath> {
    let net_class = board.net_class_for(connection.net)?;
    let route_half_width_nm = net_class.track_width_nm / 2;

    let layers: Vec<LayerId> = board.layers.iter().map(|l| l.id).collect();

    let xs = [connection.from.position.x, connection.to.position.x]
        .into_iter()
        .chain(waypoints.iter().map(|p| p.x));
    let ys = [connection.from.position.y, connection.to.position.y]
        .into_iter()
        .chain(waypoints.iter().map(|p| p.y));
    let (min_x, max_x) = xs.fold((i64::MAX, i64::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
    let (min_y, max_y) = ys.fold((i64::MAX, i64::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));

    let origin = Point::new(min_x - SEARCH_MARGIN_NM, min_y - SEARCH_MARGIN_NM);
    let far = Point::new(max_x + SEARCH_MARGIN_NM, max_y + SEARCH_MARGIN_NM);
    let width = i32::try_from((far.x - origin.x) / CELL_NM + 1).ok()?;
    let height = i32::try_from((far.y - origin.y) / CELL_NM + 1).ok()?;
    let bounds = GridBounds {
        origin,
        width,
        height,
    };

    let obstacles = obstacles_from_board(board);
    let map = ObstacleMap::build(
        board,
        &obstacles,
        connection.net,
        route_half_width_nm,
        bounds,
        &layers,
    );
    // A via's footprint is its own diameter, not the track's width, and it
    // occupies every layer it spans — a separate map, checked differently
    // (see astar::search's docs), not the same map reused with a bigger
    // number.
    let via_half_width_nm = net_class.via_diameter_nm / 2;
    let via_map = ObstacleMap::build(
        board,
        &obstacles,
        connection.net,
        via_half_width_nm,
        bounds,
        &layers,
    );

    let starts = endpoint_states(
        &connection.from.layers,
        &layers,
        connection.from.position,
        origin,
        &map,
    )?;
    let goals = endpoint_states(
        &connection.to.layers,
        &layers,
        connection.to.position,
        origin,
        &map,
    )?;

    let path = astar::search(&map, &via_map, &starts, &goals)?;
    Some(path_to_routed(&path, origin, &layers))
}

fn endpoint_states(
    endpoint_layers: &[LayerId],
    all_layers: &[LayerId],
    position: Point,
    origin: Point,
    map: &ObstacleMap,
) -> Option<Vec<(Cell, usize)>> {
    let cell = Cell::from_point(position, origin);
    if !map.in_bounds(cell) {
        return None;
    }
    let states: Vec<(Cell, usize)> = endpoint_layers
        .iter()
        .filter_map(|layer| all_layers.iter().position(|l| l == layer))
        .map(|layer_index| (cell, layer_index))
        .collect();
    if states.is_empty() {
        None
    } else {
        Some(states)
    }
}

fn path_to_routed(path: &[State], origin: Point, layers: &[LayerId]) -> RoutedPath {
    let mut segments = Vec::new();
    let mut vias = Vec::new();

    let mut run_start = 0usize;
    for i in 1..path.len() {
        if path[i].layer != path[i - 1].layer {
            emit_run_segments(
                &mut segments,
                &path[run_start..i],
                origin,
                layers[path[i - 1].layer],
            );
            vias.push(path[i - 1].cell.to_point(origin));
            run_start = i;
        }
    }
    if run_start < path.len() {
        emit_run_segments(
            &mut segments,
            &path[run_start..],
            origin,
            layers[path[run_start].layer],
        );
    }

    RoutedPath { segments, vias }
}

/// Emits one segment per straight stretch of `run` (a same-layer slice of
/// the A* path), merging consecutive grid steps only while they stay
/// exactly collinear (checked with `tessera_geom::orient`, the same exact
/// predicate `tessera-drc` uses — no epsilon, so this never merges a bend
/// into a straight line by mistake). A single segment from the run's first
/// point straight to its last would be wrong whenever the path bends
/// around an obstacle: it would silently cut the corner through whatever
/// the bend was routing around.
fn emit_run_segments(
    segments: &mut Vec<(Segment, LayerId)>,
    run: &[State],
    origin: Point,
    layer: LayerId,
) {
    if run.len() < 2 {
        return;
    }
    let mut segment_start = run[0].cell.to_point(origin);
    let mut previous = segment_start;
    for state in &run[1..] {
        let point = state.cell.to_point(origin);
        if orient(segment_start, previous, point) != Orientation::Collinear {
            segments.push((Segment::new(segment_start, previous), layer));
            segment_start = previous;
        }
        previous = point;
    }
    segments.push((Segment::new(segment_start, previous), layer));
}
