use std::collections::{HashMap, HashSet};

use tessera_geom::Point;
use tessera_global::{GCell, GlobalGrid, NetRequest};

use crate::ripup::{self, OwnRoute};
use tessera_model::{
    Board, Connection, LayerId, NetId, Pad, PadId, PadShape, Track, TrackId, Via, ViaId,
};

/// Global-grid cell size: coarser than `tessera-detail`'s 0.1mm detail
/// grid by an order of magnitude, matching plan §5.2's "coarse 3D grid"
/// framing. Not tuned against a real corpus yet.
const GLOBAL_CELL_NM: i64 = 1_000_000;
/// Margin added around the board-wide bounding box of every connection's
/// endpoints when sizing the global grid. Deliberately larger than
/// `tessera-detail`'s own `SEARCH_MARGIN_NM` (3mm) — a smaller global
/// margin would mean global waypoints could never extend a connection's
/// search window any further than its own local margin already does,
/// making the whole integration a no-op for an isolated connection.
const GLOBAL_MARGIN_NM: i64 = 5_000_000;
const MAX_NEGOTIATION_ITERATIONS: usize = 20;

#[derive(Debug, Clone, Default)]
pub struct RouteReport {
    pub routed: usize,
    /// Nets a connection existed for but `tessera-detail` couldn't find a
    /// path for within its local search window — left unrouted, not
    /// silently dropped.
    pub failed: Vec<NetId>,
    /// Notes from `Board::find_unrouted_connections` on anything this pass
    /// couldn't even attempt.
    pub skipped: Vec<String>,
    /// Plane-net surface pads given a fanout via into their plane.
    pub fanouts: usize,
    /// Plane-net surface pads no fanout via could be placed for.
    pub fanout_failed: Vec<PadId>,
}

/// Routes every unrouted net on `board`, committing each successful route
/// as new `Track`/`Via` items directly into `board` so later connections
/// see earlier ones as obstacles.
///
/// Two-pin nets route directly. Nets with three or more pads are first
/// decomposed into two-pin edges via `tessera_global::minimum_spanning_tree`
/// (plan §5.1's Steiner-decomposition step — see that function's docs for
/// why it's an MST heuristic rather than FLUTE today), then each edge
/// routes the same way a plain two-pin net would. If any edge of a
/// multi-pin net fails to route, the whole net counts as failed in the
/// report — but edges that *did* succeed stay committed rather than being
/// rolled back, so a partial failure is visible (via the report) without
/// silently discarding the progress that was made.
///
/// Before any detailed routing happens, every connection (direct or
/// Steiner-decomposed) is negotiated together on one board-wide coarse
/// grid via `tessera_global::pathfinder` (plan §5.2), and each connection's
/// negotiated path becomes a waypoint hint for `tessera-detail`'s local
/// search — see `tessera_detail::route_connection`'s docs for exactly what
/// that hint does and doesn't constrain. If global negotiation can't find
/// any coarse path for a connection at all (rare — only true grid-bounds
/// issues cause this, not congestion, per `negotiate`'s own docs), that
/// connection still gets routed with no waypoint hint, falling back to
/// `tessera-detail`'s own local window.
///
/// This is the M2/M3 baseline: sequential, single-pass, no rip-up/reroute
/// (that scheduling is M5). Connection order (both for global negotiation
/// and detailed routing) is whatever `Board::find_unrouted_connections`
/// returns (currently `HashMap` iteration order — arbitrary, and **not yet
/// deterministic**; plan §5.4 requires determinism once parallel rip-up
/// scheduling exists, but a single-threaded baseline with no scheduling
/// has nothing to be nondeterministic *about* yet beyond this ordering.
/// Fix before M5).
#[must_use]
pub fn route_board(board: &mut Board) -> RouteReport {
    let mut report = RouteReport::default();
    let connection_report = board.find_unrouted_connections();
    report.skipped = connection_report.skipped;

    // A plane net isn't routed as tracks: its pads reach the plane through
    // their own holes or a fanout via (see `fan_out_plane_pads`).
    let mut all_connections: Vec<Connection> = connection_report
        .connections
        .iter()
        .filter(|c| !board.is_plane_net(c.net))
        .cloned()
        .collect();
    let mut net_edge_counts: HashMap<NetId, usize> = HashMap::new();
    for connection in &all_connections {
        *net_edge_counts.entry(connection.net).or_insert(0) += 1;
    }
    for (net, endpoints) in &connection_report.multi_pin_nets {
        if board.is_plane_net(*net) {
            continue;
        }
        let edges = tessera_global::minimum_spanning_tree(endpoints);
        net_edge_counts.insert(*net, edges.len());
        for edge in edges {
            all_connections.push(Connection {
                net: *net,
                from: edge.from,
                to: edge.to,
            });
        }
    }

    let mut ids = Ids {
        next_track: board
            .tracks
            .iter()
            .map(|t| t.id.0)
            .max()
            .map_or(0, |m| m + 1),
        next_via: board.vias.iter().map(|v| v.id.0).max().map_or(0, |m| m + 1),
    };
    let full_span = full_layer_span(board);

    // Fanouts before global negotiation, so the coarse plan already sees
    // their vias (measured on a real board: 62 → 66 nets routed).
    let fanout_pads = pads_needing_fanout(board);
    fan_out_plane_pads(board, &fanout_pads, &mut ids, full_span, &mut report);

    let waypoints_by_connection = negotiate_global_routes(board, &all_connections);

    let no_hint = Vec::new();
    let waypoints: Vec<&[Point]> = (0..all_connections.len())
        .map(|i| {
            waypoints_by_connection
                .get(i)
                .unwrap_or(&no_hint)
                .as_slice()
        })
        .collect();
    let mut routes: Vec<Option<OwnRoute>> = all_connections
        .iter()
        .zip(&waypoints)
        .map(|(c, w)| route_and_commit(board, c, w, &mut ids, full_span))
        .collect();
    rip_up_and_reroute(
        board,
        &all_connections,
        &waypoints,
        &mut routes,
        &mut ids,
        full_span,
    );

    let mut net_success_counts: HashMap<NetId, usize> = HashMap::new();
    for (connection, route) in all_connections.iter().zip(&routes) {
        if route.is_some() {
            *net_success_counts.entry(connection.net).or_insert(0) += 1;
        }
    }

    for (net, total) in net_edge_counts {
        let succeeded = net_success_counts.get(&net).copied().unwrap_or(0);
        if succeeded == total {
            report.routed += 1;
        } else {
            report.failed.push(net);
        }
    }
    // `net_edge_counts` is a `HashMap`, so `report.failed`'s order is
    // otherwise as arbitrary as the routing order itself (see
    // `Board::find_unrouted_connections`'s docs) — this doesn't change
    // *which* nets fail, only makes the report's own listing order
    // reproducible.
    report.failed.sort();

    report
}

/// Builds a board-wide coarse grid, runs `tessera_global::pathfinder`'s
/// negotiated congestion across every connection at once, and returns each
/// connection's negotiated path converted to real-coordinate waypoints (in
/// `all_connections` order; a missing entry means no coarse path was
/// found at all — see this function's caller for how that's handled).
///
/// The grid's per-cell capacity is reduced by real fixed board geometry —
/// see `obstruction_from_board` — so a waypoint hint can now steer a
/// connection *around* a specific obstacle, not just negotiate its route
/// relative to competing nets. When `board.outline` is known (see
/// `apply_outline_obstruction`), every cell outside it is fully blocked
/// too — but this is still only a *soft* global waypoint guide: a
/// connection whose global path can't be found (e.g. because the outline
/// blocks it entirely) falls back to `tessera-detail`'s unconstrained
/// full-window search, which knows nothing about the outline at all —
/// see that function's own docs. **Not a hard outline-enforcement
/// guarantee at the detail-routing level yet.** Still not obstacle-aware:
/// via-edge capacity (`GlobalGrid`'s own docs).
fn negotiate_global_routes(board: &Board, all_connections: &[Connection]) -> Vec<Vec<Point>> {
    let layers = board.signal_layers();
    if all_connections.is_empty() || layers.is_empty() {
        return Vec::new();
    }

    let mut min_x = i64::MAX;
    let mut max_x = i64::MIN;
    let mut min_y = i64::MAX;
    let mut max_y = i64::MIN;
    for connection in all_connections {
        for point in [connection.from.position, connection.to.position] {
            min_x = min_x.min(point.x);
            max_x = max_x.max(point.x);
            min_y = min_y.min(point.y);
            max_y = max_y.max(point.y);
        }
    }
    let origin = Point::new(min_x - GLOBAL_MARGIN_NM, min_y - GLOBAL_MARGIN_NM);
    let far = Point::new(max_x + GLOBAL_MARGIN_NM, max_y + GLOBAL_MARGIN_NM);
    let Ok(width) = i32::try_from((far.x - origin.x) / GLOBAL_CELL_NM + 1) else {
        return Vec::new();
    };
    let Ok(height) = i32::try_from((far.y - origin.y) / GLOBAL_CELL_NM + 1) else {
        return Vec::new();
    };

    // A single board-wide pitch estimate (see module docs): the largest
    // track_width + clearance across every net class present, so no net's
    // real requirement is underestimated by a more generous class's stats.
    let pitch_nm = board
        .net_classes
        .values()
        .map(|c| c.track_width_nm + c.clearance_nm)
        .max()
        .unwrap_or(GLOBAL_CELL_NM);
    let capacity_per_layer = (GLOBAL_CELL_NM / pitch_nm.max(1)).max(1);
    let mut grid = GlobalGrid {
        origin,
        cell_size_nm: GLOBAL_CELL_NM,
        width,
        height,
        layer_capacity: vec![capacity_per_layer; layers.len()],
        obstruction: Vec::new(),
    };
    let active_nets: HashSet<NetId> = all_connections.iter().map(|c| c.net).collect();
    grid.obstruction = obstruction_from_board(board, &grid, &layers, &active_nets);

    let requests: Vec<NetRequest> = all_connections
        .iter()
        .map(|connection| NetRequest {
            starts: endpoint_cells(
                &grid,
                &layers,
                &connection.from.layers,
                connection.from.position,
            ),
            goals: endpoint_cells(
                &grid,
                &layers,
                &connection.to.layers,
                connection.to.position,
            ),
        })
        .collect();

    let result = tessera_global::negotiate(&grid, &requests, MAX_NEGOTIATION_ITERATIONS);

    result
        .paths
        .into_iter()
        .map(|path| {
            path.map(|cells| {
                dedup_points(
                    &cells
                        .into_iter()
                        .map(|c| grid.point_of(c))
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default()
        })
        .collect()
}

/// Rasterizes every fixed board obstacle (existing tracks, vias, pads —
/// the same source `tessera_detail::ObstacleMap` uses at the fine grid)
/// onto `grid`'s coarse resolution: a cell whose centre falls inside an
/// obstacle's copper has its full per-layer capacity marked consumed,
/// pushing negotiation to route around it. Obstacles on a net in
/// `active_nets` are skipped — those are the nets being negotiated this
/// round, so their own pins would otherwise obstruct their own starting
/// cell on every single run. The one accuracy cost of that: if a net in
/// `active_nets` has other, already-committed copper elsewhere on the
/// board (e.g. one edge of a multi-pin net routed, another still
/// pending), that copper isn't treated as an obstacle for this round
/// either — acceptable since this grid is a soft waypoint hint, not a
/// hard constraint, and `tessera-detail` still checks real clearance
/// against it regardless.
fn obstruction_from_board(
    board: &Board,
    grid: &GlobalGrid,
    layers: &[LayerId],
    active_nets: &HashSet<NetId>,
) -> Vec<i64> {
    let layer_count = layers.len();
    let Ok(width) = usize::try_from(grid.width) else {
        return Vec::new();
    };
    let Ok(height) = usize::try_from(grid.height) else {
        return Vec::new();
    };
    let mut obstruction = vec![0i64; width * height * layer_count];

    for obstacle in tessera_detail::obstacles_from_board(board) {
        if active_nets.contains(&obstacle.net)
            || obstacle.blocks == tessera_detail::Blocks::ViasOnly
        {
            continue;
        }
        let Some(layer_index) = layers.iter().position(|&l| l == obstacle.layer) else {
            continue;
        };
        let full_capacity = grid.layer_capacity.get(layer_index).copied().unwrap_or(0);
        if full_capacity == 0 {
            continue;
        }
        let (min_pt, max_pt) = obstacle.shape.bounding_box(0);
        let min_cell = grid.cell_of(min_pt, layer_index);
        let max_cell = grid.cell_of(max_pt, layer_index);

        for cy in min_cell.y.max(0)..=max_cell.y.min(grid.height - 1) {
            for cx in min_cell.x.max(0)..=max_cell.x.min(grid.width - 1) {
                let cell = GCell {
                    x: cx,
                    y: cy,
                    layer: layer_index,
                };
                let point = grid.point_of(cell);
                if !obstacle.shape.clears_point(point, 0) {
                    if let Some(idx) = grid.cell_index(cell) {
                        obstruction[idx] = obstruction[idx].max(full_capacity);
                    }
                }
            }
        }
    }

    if let Some(outline) = &board.outline {
        apply_outline_obstruction(outline, grid, layer_count, width, height, &mut obstruction);
    }

    obstruction
}

/// Fully blocks every cell (on every layer) whose centre point falls
/// outside `outline` — a board's `Edge.Cuts` boundary is a hard routing
/// limit, not a soft preference, so this uses each layer's *full*
/// capacity the same way an unavoidable copper obstacle does, not a
/// partial reduction. Only ever tightens `obstruction`, never loosens it
/// (a cell already fully blocked by real geometry stays blocked
/// regardless of the outline).
///
/// `Board::outline` is `None` — this function isn't called at all — for
/// any board `tessera-io-kicad::parser::parse_board_outline` couldn't
/// build one for (an arc-containing outline, or a set of `Edge.Cuts`
/// graphics that don't form a single closed loop — see that function's
/// docs), so a board with no outline known routes exactly as before this
/// feature existed: unrestricted by any outline at all.
fn apply_outline_obstruction(
    outline: &tessera_geom::Polygon,
    grid: &GlobalGrid,
    layer_count: usize,
    width: usize,
    height: usize,
    obstruction: &mut [i64],
) {
    for layer_index in 0..layer_count {
        let full_capacity = grid.layer_capacity.get(layer_index).copied().unwrap_or(0);
        if full_capacity == 0 {
            continue;
        }
        for cy in 0..height {
            for cx in 0..width {
                let Ok(x) = i32::try_from(cx) else { continue };
                let Ok(y) = i32::try_from(cy) else { continue };
                let cell = GCell {
                    x,
                    y,
                    layer: layer_index,
                };
                let point = grid.point_of(cell);
                if !outline.contains_point(point) {
                    if let Some(idx) = grid.cell_index(cell) {
                        obstruction[idx] = obstruction[idx].max(full_capacity);
                    }
                }
            }
        }
    }
}

fn endpoint_cells(
    grid: &GlobalGrid,
    all_layers: &[LayerId],
    endpoint_layers: &[LayerId],
    position: Point,
) -> Vec<GCell> {
    endpoint_layers
        .iter()
        .filter_map(|layer| all_layers.iter().position(|l| l == layer))
        .map(|layer_index| grid.cell_of(position, layer_index))
        .collect()
}

/// Collapses consecutive duplicate points (a layer-change step in the
/// global path moves between layers at the same (x, y), which would
/// otherwise add a zero-length "waypoint" that does nothing but pad the
/// list `tessera-detail` folds into its bounding box).
fn dedup_points(points: &[Point]) -> Vec<Point> {
    let mut result: Vec<Point> = Vec::with_capacity(points.len());
    for &point in points {
        if result.last() != Some(&point) {
            result.push(point);
        }
    }
    result
}

/// Routes one two-pin `connection` via `tessera-detail` (with `waypoints`
/// as a search-window hint from global routing) and, on success, commits
/// the result into `board` as new `Track`/`Via` items. Returns whether it
/// succeeded.
/// Plan §5.4's progressive rip-up, on top of the single first pass: for
/// each still-failed connection (in order — deterministic), first just
/// retry it; failing that, plan it on a board without this run's own
/// routes, rip up exactly the own routes in that plan's way, route it, and
/// reroute those. The attempt is kept only if strictly more connections end
/// up routed; otherwise the board is restored. Repeats while a pass
/// improves anything, up to `ripup::MAX_PASSES`.
fn rip_up_and_reroute(
    board: &mut Board,
    connections: &[Connection],
    waypoints: &[&[Point]],
    routes: &mut [Option<OwnRoute>],
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
) {
    let routed_count = |routes: &[Option<OwnRoute>]| routes.iter().filter(|r| r.is_some()).count();
    for _ in 0..ripup::MAX_PASSES {
        let mut improved = false;
        for f in 0..connections.len() {
            if routes[f].is_some() {
                continue;
            }
            if let Some(route) =
                route_and_commit(board, &connections[f], waypoints[f], ids, full_span)
            {
                routes[f] = Some(route);
                improved = true;
                continue;
            }
            let ghost_board = ripup::without_own_routes(board, routes);
            let Some(ghost) =
                tessera_detail::route_connection(&ghost_board, &connections[f], waypoints[f])
            else {
                continue; // unroutable even with none of this run's own routes in the way
            };
            let blockers = ripup::blockers(board, connections, routes, &ghost, connections[f].net);
            if blockers.is_empty() || blockers.len() > ripup::MAX_BLOCKERS {
                continue;
            }

            let before = routed_count(routes);
            let snapshot = (board.clone(), routes.to_vec(), ids.clone());
            for &b in &blockers {
                if let Some(route) = routes[b].take() {
                    ripup::remove(board, &route);
                }
            }
            routes[f] = route_and_commit(board, &connections[f], waypoints[f], ids, full_span);
            for &b in &blockers {
                routes[b] = route_and_commit(board, &connections[b], waypoints[b], ids, full_span);
            }
            if routed_count(routes) > before {
                improved = true;
            } else {
                let (old_board, old_routes, old_ids) = snapshot;
                *board = old_board;
                routes.clone_from_slice(&old_routes);
                *ids = old_ids;
            }
        }
        if !improved {
            break;
        }
    }
}

#[derive(Debug, Clone)]
struct Ids {
    next_track: u32,
    next_via: u32,
}

fn route_and_commit(
    board: &mut Board,
    connection: &Connection,
    waypoints: &[Point],
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
) -> Option<OwnRoute> {
    let routed = tessera_detail::route_connection(board, connection, waypoints)?;
    commit(board, connection.net, &routed, ids, full_span)
}

/// Adds `routed`'s tracks (net-class width) and through vias
/// (`Board::via_size_for`) to `board`.
fn commit(
    board: &mut Board,
    net: NetId,
    routed: &tessera_detail::RoutedPath,
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
) -> Option<OwnRoute> {
    let width_nm = board.net_class_for(net)?.track_width_nm;
    let (via_diameter_nm, via_drill_nm) = board.via_size_for(net)?;
    let mut own = OwnRoute::default();
    for (segment, layer) in &routed.segments {
        own.tracks.push(TrackId(ids.next_track));
        board.tracks.push(Track {
            id: TrackId(ids.next_track),
            segment: *segment,
            width_nm,
            layer: *layer,
            net,
            locked: false,
        });
        ids.next_track += 1;
    }
    for position in &routed.vias {
        own.vias.push(ViaId(ids.next_via));
        board.vias.push(Via {
            id: ViaId(ids.next_via),
            position: *position,
            diameter_nm: via_diameter_nm,
            drill_nm: via_drill_nm,
            from_layer: full_span.0,
            to_layer: full_span.1,
            net,
            locked: false,
        });
        ids.next_via += 1;
    }
    Some(own)
}

/// Gives every surface pad of a plane net that has no copper of its own
/// net touching it yet a short track and via into its plane. Through-hole
/// pads already reach every plane through their hole; a pad the user has
/// already wired up (a track ending on it, or a via in it) is left alone.
fn pads_needing_fanout(board: &Board) -> Vec<Pad> {
    let signal_layers = board.signal_layers();
    board
        .pads
        .iter()
        .filter(|p| board.is_plane_net(p.net))
        .filter(|p| p.layers.iter().all(|l| signal_layers.contains(l)))
        .filter(|p| !has_own_copper(board, p))
        .cloned()
        .collect()
}

/// Fans each of `pads` out into its plane.
fn fan_out_plane_pads(
    board: &mut Board,
    pads: &[Pad],
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
    report: &mut RouteReport,
) {
    for pad in pads {
        match tessera_detail::fanout_to_plane(board, pad) {
            Some(routed) if commit(board, pad.net, &routed, ids, full_span).is_some() => {
                report.fanouts += 1;
            }
            _ => report.fanout_failed.push(pad.id),
        }
    }
}

fn has_own_copper(board: &Board, pad: &Pad) -> bool {
    let touches = |p: Point| match &pad.shape {
        PadShape::Circle(c) => !c.clears_point(p, 1),
        PadShape::Rect(r) => !r.clears_point(p, 1),
        PadShape::Oval(o) => !o.clears_point(p, 1),
    };
    board.tracks.iter().any(|t| {
        t.net == pad.net
            && pad.layers.contains(&t.layer)
            && (touches(t.segment.a) || touches(t.segment.b))
    }) || board
        .vias
        .iter()
        .any(|v| v.net == pad.net && touches(v.position))
}

fn full_layer_span(board: &Board) -> (LayerId, LayerId) {
    let mut ids: Vec<LayerId> = board.layers.iter().map(|l| l.id).collect();
    ids.sort_by_key(|l| l.0);
    match (ids.first(), ids.last()) {
        (Some(&first), Some(&last)) => (first, last),
        _ => (LayerId(0), LayerId(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::{Circle, Segment};
    use tessera_model::{Net, NetClass, Pad, PadId, PadShape};

    fn two_layer_board() -> Board {
        let mut board = Board::new();
        board
            .layers
            .push(tessera_model::Layer::copper(LayerId(0), "F.Cu"));
        board
            .layers
            .push(tessera_model::Layer::copper(LayerId(1), "B.Cu"));
        board.net_classes.insert(
            "Default".to_string(),
            NetClass {
                name: "Default".to_string(),
                clearance_nm: 150_000,
                track_width_nm: 200_000,
                via_diameter_nm: 500_000,
                via_drill_nm: 250_000,
                diff_pair_track_width_nm: None,
                diff_pair_gap_nm: None,
                diff_pair_via_gap_nm: None,
            },
        );
        board
    }

    #[test]
    fn foreign_copper_obstructs_a_cell_but_an_active_nets_own_copper_does_not() {
        let mut board = two_layer_board();

        let blocker_net = NetId(1);
        board.nets.insert(
            blocker_net,
            Net {
                id: blocker_net,
                name: "BLOCKER".to_string(),
                net_class: "Default".to_string(),
            },
        );
        // Wide enough (2mm) to fully cover the 1mm cell centred at (0, 0).
        board.tracks.push(Track {
            id: TrackId(0),
            segment: Segment::new(Point::new(-1_000_000, 0), Point::new(1_000_000, 0)),
            width_nm: 2_000_000,
            layer: LayerId(0),
            net: blocker_net,
            locked: true,
        });

        let active_net = NetId(2);
        board.nets.insert(
            active_net,
            Net {
                id: active_net,
                name: "ACTIVE".to_string(),
                net_class: "Default".to_string(),
            },
        );
        // Placed exactly on a cell centre so, if it were rasterized, it
        // would obstruct that cell just as fully as the blocker track does.
        board.pads.push(Pad {
            id: PadId(0),
            shape: PadShape::Circle(Circle::new(Point::new(3_000_000, 3_000_000), 500_000)),
            layers: vec![LayerId(0)],
            net: active_net,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        });

        let grid = GlobalGrid {
            origin: Point::new(-2_000_000, -2_000_000),
            cell_size_nm: GLOBAL_CELL_NM,
            width: 8,
            height: 8,
            layer_capacity: vec![4, 4],
            obstruction: Vec::new(),
        };
        let layers = vec![LayerId(0), LayerId(1)];
        let active_nets: HashSet<NetId> = [active_net].into_iter().collect();

        let obstruction = obstruction_from_board(&board, &grid, &layers, &active_nets);

        let blocker_cell = grid.cell_of(Point::new(0, 0), 0);
        let blocker_idx = grid.cell_index(blocker_cell).unwrap();
        assert_eq!(
            obstruction[blocker_idx], 4,
            "foreign locked copper should fully consume this cell's capacity"
        );

        let active_cell = grid.cell_of(Point::new(3_000_000, 3_000_000), 0);
        let active_idx = grid.cell_index(active_cell).unwrap();
        assert_eq!(
            obstruction[active_idx], 0,
            "an active net's own copper must not obstruct its own cell"
        );
    }

    #[test]
    fn a_cell_outside_the_board_outline_is_fully_obstructed_on_every_layer() {
        let mut board = two_layer_board();
        // A 4mm x 4mm square outline, centred on the grid built below.
        board.outline = Some(tessera_geom::Polygon::new(vec![
            Point::new(0, 0),
            Point::new(4_000_000, 0),
            Point::new(4_000_000, 4_000_000),
            Point::new(0, 4_000_000),
        ]));

        let grid = GlobalGrid {
            origin: Point::new(-2_000_000, -2_000_000),
            cell_size_nm: GLOBAL_CELL_NM,
            width: 8,
            height: 8,
            layer_capacity: vec![4, 4],
            obstruction: Vec::new(),
        };
        let layers = vec![LayerId(0), LayerId(1)];
        let active_nets: HashSet<NetId> = HashSet::new();

        let obstruction = obstruction_from_board(&board, &grid, &layers, &active_nets);

        // (-1.5mm, -1.5mm) sits well outside the outline, on both layers.
        let outside_cell_l0 = grid.cell_of(Point::new(-1_500_000, -1_500_000), 0);
        let outside_cell_l1 = grid.cell_of(Point::new(-1_500_000, -1_500_000), 1);
        assert_eq!(
            obstruction[grid.cell_index(outside_cell_l0).unwrap()],
            4,
            "a cell outside the outline must be fully blocked on layer 0"
        );
        assert_eq!(
            obstruction[grid.cell_index(outside_cell_l1).unwrap()],
            4,
            "a cell outside the outline must be fully blocked on layer 1 too"
        );

        // (2mm, 2mm) sits well inside the outline, away from any real
        // copper obstacle, so it should be untouched.
        let inside_cell = grid.cell_of(Point::new(2_000_000, 2_000_000), 0);
        assert_eq!(
            obstruction[grid.cell_index(inside_cell).unwrap()],
            0,
            "a cell inside the outline must not be obstructed by the outline"
        );
    }

    #[test]
    fn a_board_with_no_known_outline_obstructs_nothing_extra() {
        // board.outline defaults to None — obstruction_from_board must
        // behave exactly as it did before this feature existed.
        let board = two_layer_board();
        assert!(board.outline.is_none());

        let grid = GlobalGrid {
            origin: Point::new(-2_000_000, -2_000_000),
            cell_size_nm: GLOBAL_CELL_NM,
            width: 8,
            height: 8,
            layer_capacity: vec![4, 4],
            obstruction: Vec::new(),
        };
        let layers = vec![LayerId(0), LayerId(1)];
        let obstruction = obstruction_from_board(&board, &grid, &layers, &HashSet::new());
        assert!(obstruction.iter().all(|&c| c == 0));
    }
}
