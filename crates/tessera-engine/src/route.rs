use std::collections::{HashMap, HashSet};

use tessera_geom::Point;
use tessera_global::{GCell, GlobalGrid, NetRequest};

use crate::negotiate;
use crate::pair;
use crate::ripup::{self, OwnRoute};
use tessera_model::{
    Board, Connection, Endpoint, LayerId, NetId, Pad, PadId, PadShape, Track, TrackId, Via, ViaId,
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
    /// Every connection left unrouted: its net and the two ends.
    pub failed_links: Vec<(NetId, Point, Point)>,
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
        .filter(|c| !board.is_plane_net(c.net) && !board.is_pour_net(c.net))
        .cloned()
        .collect();
    for (net, endpoints) in &connection_report.multi_pin_nets {
        if board.is_plane_net(*net) || board.is_pour_net(*net) {
            continue;
        }
        let edges = tessera_global::minimum_spanning_tree(endpoints);
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
    // Two complete routings on copies of the board — the sequential one,
    // and one seeded by negotiated congestion — and whichever routes more
    // connections is kept (the sequential one on a tie). Neither strategy
    // wins everywhere; keeping the better one is never worse than either.
    let sequential = route_strategy(
        board,
        &all_connections,
        &waypoints,
        ids.clone(),
        full_span,
        false,
    );
    let negotiated = route_strategy(
        board,
        &all_connections,
        &waypoints,
        ids.clone(),
        full_span,
        true,
    );
    let failures = |r: &Strategy| r.routes.iter().filter(|x| x.is_none()).count();
    let chosen = if failures(&negotiated) < failures(&sequential) {
        negotiated
    } else {
        sequential
    };
    *board = chosen.board;
    let all_connections = chosen.connections;
    let mut routes = chosen.routes;
    let paired = chosen.paired;
    // Each strategy advanced its own copy of the id counters; continue
    // from what the chosen board actually holds — and never below where
    // routing started, so an id of copper rip-up took out isn't reused.
    let mut ids = ids_after(board, &ids);
    optimize(
        board,
        &all_connections,
        &mut routes,
        &mut ids,
        full_span,
        &paired,
    );
    stitch_pour_islands(board, &all_connections, &mut routes, &mut ids, full_span);
    rejoin_elsewhere(board, &all_connections, &mut routes, &mut ids, full_span);

    count_nets(&all_connections, &routes, &mut report);
    report.failed_links = all_connections
        .iter()
        .zip(&routes)
        .filter(|(_, r)| r.is_none())
        .map(|(c, _)| (c.net, c.from.position, c.to.position))
        .collect();
    // `count_nets` works over a `HashMap`, so `report.failed`'s order is
    // otherwise as arbitrary as the routing order itself (see
    // `Board::find_unrouted_connections`'s docs) — this doesn't change
    // *which* nets fail, only makes the report's own listing order
    // reproducible.
    report.failed.sort();

    report
}

/// For each link still unrouted between two islands of a poured net (one
/// with zone fills): a stitching via instead — from the pad at one end,
/// a short track to a via landing on a fill, on another layer, of the
/// other end's island. What a designer does with a pad cut off from its
/// pour on one side while the pour on the other side runs right under it.
fn stitch_pour_islands(
    board: &mut Board,
    connections: &[Connection],
    routes: &mut [Option<OwnRoute>],
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
) {
    for (i, connection) in connections.iter().enumerate() {
        if routes[i].is_some() || !board.zone_fills.iter().any(|f| f.net == connection.net) {
            continue;
        }
        let path = overlap_stitch(board, connection).or_else(|| pad_stitch(board, connection));
        if let Some(path) = path {
            routes[i] = commit(board, connection.net, &path, ids, full_span);
        }
    }
}

/// Spacing of the points sampled inside a pour island as route ends.
const FILL_POINT_STEP_NM: i64 = 500_000;
/// How far from the other island's ends those samples are taken.
const FILL_POINT_REACH_NM: i64 = 6_000_000;

/// Points well inside (a track's width and clearance) the fills joined to
/// the copper at `at`, within reach of any of `towards` — places a link
/// may start from on a pour island instead of a pad.
fn fill_points(
    board: &Board,
    net: NetId,
    at: Point,
    towards: &[tessera_model::Endpoint],
) -> Vec<tessera_model::Endpoint> {
    let margin = board.track_width_for(net).unwrap_or(0)
        + board.net_class_for(net).map_or(0, |c| c.clearance_nm);
    let mut points = Vec::new();
    for k in tessera_model::connectivity::fills_joined_to(board, net, at) {
        let fill = &board.zone_fills[k];
        let (lo, hi) = fill.polygon.bounding_box(0);
        let near = |p: Point| {
            towards.iter().any(|t| {
                (t.position.x - p.x).abs() <= FILL_POINT_REACH_NM
                    && (t.position.y - p.y).abs() <= FILL_POINT_REACH_NM
            })
        };
        let inside = |p: Point| {
            [(0, 0), (margin, 0), (-margin, 0), (0, margin), (0, -margin)]
                .iter()
                .all(|&(dx, dy)| fill.polygon.contains_point(Point::new(p.x + dx, p.y + dy)))
        };
        let mut y = lo.y - lo.y.rem_euclid(FILL_POINT_STEP_NM);
        while y <= hi.y {
            let mut x = lo.x - lo.x.rem_euclid(FILL_POINT_STEP_NM);
            while x <= hi.x {
                let p = Point::new(x, y);
                if near(p) && inside(p) {
                    points.push(tessera_model::Endpoint {
                        position: p,
                        layers: vec![fill.layer],
                    });
                }
                x += FILL_POINT_STEP_NM;
            }
            y += FILL_POINT_STEP_NM;
        }
    }
    points
}

/// How many other anchor pairs to try for a link between two islands.
const OTHER_ANCHOR_TRIES: usize = 8;

/// For each link still unrouted between two islands of copper already on
/// the board: the islands may also join elsewhere — any pad, via or track
/// end of one to any of the other. Tries the nearest other pairs.
fn rejoin_elsewhere(
    board: &mut Board,
    connections: &[Connection],
    routes: &mut [Option<OwnRoute>],
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
) {
    for (i, connection) in connections.iter().enumerate() {
        if routes[i].is_some() {
            continue;
        }
        let islands = tessera_model::connectivity::pad_islands(board, connection.net);
        let island_of = |p: Point| {
            islands
                .iter()
                .position(|isl| isl.anchors.iter().any(|a| a.position == p))
        };
        let (Some(ia), Some(ib)) = (
            island_of(connection.from.position),
            island_of(connection.to.position),
        ) else {
            continue;
        };
        if ia == ib {
            continue;
        }
        let d2 = |a: Point, b: Point| i128::from(a.x - b.x).pow(2) + i128::from(a.y - b.y).pow(2);
        // a route may also start or end anywhere well inside an island's pour
        let mut ends_a = islands[ia].anchors.clone();
        let mut ends_b = islands[ib].anchors.clone();
        ends_a.extend(fill_points(
            board,
            connection.net,
            connection.from.position,
            &ends_b,
        ));
        ends_b.extend(fill_points(
            board,
            connection.net,
            connection.to.position,
            &ends_a,
        ));
        let mut pairs: Vec<(&tessera_model::Endpoint, &tessera_model::Endpoint)> = ends_a
            .iter()
            .flat_map(|a| ends_b.iter().map(move |b| (a, b)))
            .filter(|(a, b)| {
                !(a.position == connection.from.position && b.position == connection.to.position)
            })
            .collect();
        pairs.sort_by_key(|(a, b)| d2(a.position, b.position));
        pairs.dedup_by_key(|(a, b)| (a.position, b.position));
        let tries: Vec<Connection> = pairs
            .into_iter()
            .take(OTHER_ANCHOR_TRIES)
            .map(|(a, b)| Connection {
                net: connection.net,
                from: a.clone(),
                to: b.clone(),
            })
            .collect();
        for link in &tries {
            if let Some(route) = route_and_commit(board, link, &[], ids, full_span) {
                routes[i] = Some(route);
                break;
            }
        }
    }
}

/// A lone via where a fill of one end's island overlaps, on another layer,
/// a fill of the other end's — well inside both (its whole pad on each).
fn overlap_stitch(board: &Board, connection: &Connection) -> Option<tessera_detail::RoutedPath> {
    let net = connection.net;
    let joined = |at: Point| -> Vec<&tessera_model::ZoneFill> {
        tessera_model::connectivity::fills_joined_to(board, net, at)
            .into_iter()
            .map(|k| &board.zone_fills[k])
            .collect()
    };
    let (ours, theirs) = (
        joined(connection.from.position),
        joined(connection.to.position),
    );
    let radius = board.via_size_for(net)?.0 / 2;
    let covers = |f: &tessera_model::ZoneFill, p: Point| {
        [(0, 0), (radius, 0), (-radius, 0), (0, radius), (0, -radius)]
            .iter()
            .all(|&(dx, dy)| f.polygon.contains_point(Point::new(p.x + dx, p.y + dy)))
    };
    for a in &ours {
        for b in theirs.iter().filter(|b| b.layer != a.layer) {
            let (a_lo, a_hi) = a.polygon.bounding_box(0);
            let (b_lo, b_hi) = b.polygon.bounding_box(0);
            let lo = Point::new(a_lo.x.max(b_lo.x), a_lo.y.max(b_lo.y));
            let hi = Point::new(a_hi.x.min(b_hi.x), a_hi.y.min(b_hi.y));
            if lo.x > hi.x || lo.y > hi.y {
                continue;
            }
            let accept = |p: Point| covers(a, p) && covers(b, p);
            if let Some(via) = tessera_detail::via_site(board, net, lo, hi, &accept) {
                return Some(tessera_detail::RoutedPath {
                    segments: Vec::new(),
                    vias: vec![via],
                    necks: Vec::new(),
                });
            }
        }
    }
    None
}

/// From the pad at one end, a short track to a via landing on a fill, on
/// another layer, of the other end's island.
fn pad_stitch(board: &Board, connection: &Connection) -> Option<tessera_detail::RoutedPath> {
    for (near, far) in [
        (&connection.from, &connection.to),
        (&connection.to, &connection.from),
    ] {
        let Some(pad) = board
            .pads
            .iter()
            .find(|p| p.net == connection.net && p.shape.center() == near.position)
        else {
            continue;
        };
        let fills: Vec<&tessera_model::ZoneFill> =
            tessera_model::connectivity::fills_joined_to(board, connection.net, far.position)
                .into_iter()
                .map(|k| &board.zone_fills[k])
                .filter(|f| !pad.layers.contains(&f.layer))
                .collect();
        if fills.is_empty() {
            continue;
        }
        let on_fill = |p: Point| fills.iter().any(|f| f.polygon.contains_point(p));
        if let Some(path) = tessera_detail::fanout_into(board, pad, &on_fill) {
            return Some(path);
        }
    }
    None
}

/// Per net: routed if every one of its connections is (the requested ones,
/// plus links of nets rip-up took apart), failed otherwise.
fn count_nets(connections: &[Connection], routes: &[Option<OwnRoute>], report: &mut RouteReport) {
    let mut per_net: HashMap<NetId, (usize, usize)> = HashMap::new();
    for (connection, route) in connections.iter().zip(routes) {
        let entry = per_net.entry(connection.net).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += usize::from(route.is_some());
    }
    for (net, (total, succeeded)) in per_net {
        if succeeded == total {
            report.routed += 1;
        } else {
            report.failed.push(net);
        }
    }
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
/// routes — free to cross them but at a price ([`ripup::OwnCopper`]), and
/// over the whole window rather than the coarse corridor, so the plan
/// crosses as few of them as it must — rip up exactly the own routes in
/// that plan's way, route it, and reroute those. The attempt is kept only if strictly more connections end
/// up routed; otherwise the board is restored. Repeats while a pass
/// improves anything, up to `ripup::MAX_PASSES`.
fn rip_up_and_reroute(
    board: &mut Board,
    connections: &mut Vec<Connection>,
    waypoints: &mut Vec<&[Point]>,
    routes: &mut Vec<Option<OwnRoute>>,
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
) {
    let failures = |routes: &[Option<OwnRoute>]| routes.iter().filter(|r| r.is_none()).count();
    for _ in 0..ripup::MAX_PASSES {
        let mut improved = false;
        let mut f = 0;
        while f < connections.len() {
            if routes[f].is_some() {
                f += 1;
                continue;
            }
            if let Some(route) =
                route_and_commit(board, &connections[f], waypoints[f], ids, full_span)
            {
                routes[f] = Some(route);
                improved = true;
                f += 1;
                continue;
            }
            if rip_up_for(
                board,
                connections,
                waypoints,
                routes,
                ids,
                full_span,
                f,
                failures,
            ) {
                improved = true;
            }
            f += 1;
        }
        if !improved {
            break;
        }
    }
}

/// One rip-up attempt for the failed connection `f` (see
/// [`rip_up_and_reroute`]): plans it as if this run's routes and the
/// reroutable copper weren't there, rips up what that plan hits — whole
/// routes of this run, and per net the reroutable copper in the way (that
/// net's broken links then join `connections`) — and routes it all again.
/// Kept, returning true, only if fewer connections end up unrouted.
#[allow(clippy::too_many_arguments)]
fn rip_up_for(
    board: &mut Board,
    connections: &mut Vec<Connection>,
    waypoints: &mut Vec<&[Point]>,
    routes: &mut Vec<Option<OwnRoute>>,
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
    f: usize,
    failures: impl Fn(&[Option<OwnRoute>]) -> usize,
) -> bool {
    let net = connections[f].net;
    let ghost_board = ripup::without_own_routes(board, routes);
    let margin = board.track_width_for(net).unwrap_or(0) / 2
        + board.net_class_for(net).map_or(0, |c| c.clearance_nm);
    let own = ripup::OwnCopper::new(board, routes, margin);
    let Some(ghost) =
        tessera_detail::route_connection_with_cost(&ghost_board, &connections[f], &[], Some(&own))
    else {
        return false; // unroutable even with none of the rip-up-able copper in the way
    };
    let blockers = ripup::blockers(board, connections, routes, &ghost, net);
    let released = ripup::reroutable_blockers(board, &ghost, net);
    let count = blockers.len() + released.len();
    if count == 0 || count > ripup::MAX_BLOCKERS {
        return false;
    }

    let before = failures(routes);
    let snapshot = (board.clone(), routes.clone(), ids.clone());
    let (n_connections, n_waypoints) = (connections.len(), waypoints.len());
    for &b in &blockers {
        if let Some(route) = routes[b].take() {
            ripup::remove(board, &route);
        }
    }
    let mut links = Vec::new();
    for (released_net, items) in &released {
        links.extend(ripup::release(board, *released_net, items));
    }
    routes[f] = route_and_commit(board, &connections[f], waypoints[f], ids, full_span);
    for &b in &blockers {
        routes[b] = route_and_commit(board, &connections[b], waypoints[b], ids, full_span);
    }
    for link in links {
        let route = route_and_commit(board, &link, &[], ids, full_span);
        connections.push(link);
        waypoints.push(&[]);
        routes.push(route);
    }
    if failures(routes) < before {
        return true;
    }
    let (old_board, old_routes, old_ids) = snapshot;
    *board = old_board;
    *routes = old_routes;
    *ids = old_ids;
    connections.truncate(n_connections);
    waypoints.truncate(n_waypoints);
    false
}

/// One complete routing of `connections` on a copy of `board`, followed by
/// rip-up: either plain sequential routing, or first committing every
/// negotiated plan (`negotiate::plan`) that is legal as-is — in connection
/// order, each exactly checked — and routing the rest sequentially.
fn route_strategy(
    board: &Board,
    connections: &[Connection],
    waypoints: &[&[Point]],
    mut ids: Ids,
    full_span: (LayerId, LayerId),
    negotiated: bool,
) -> Strategy {
    let mut board = board.clone();
    let mut connections = connections.to_vec();
    let mut waypoints = waypoints.to_vec();
    let planned = if negotiated {
        negotiate::plan(&board, &connections)
    } else {
        vec![None; connections.len()]
    };
    let mut routes: Vec<Option<OwnRoute>> = vec![None; connections.len()];
    // diff pairs first, side by side, while there's room
    let paired = route_pairs_together(&mut board, &connections, &mut routes, &mut ids, full_span);
    for (i, connection) in connections.iter().enumerate() {
        if routes[i].is_some() {
            continue;
        }
        if let Some(path) = &planned[i] {
            if tessera_detail::path_is_legal(&board, connection.net, path) {
                routes[i] = commit(&mut board, connection.net, path, &mut ids, full_span);
            }
        }
    }
    for (i, connection) in connections.iter().enumerate() {
        if routes[i].is_none() {
            routes[i] = route_and_commit(&mut board, connection, waypoints[i], &mut ids, full_span);
        }
    }
    rip_up_and_reroute(
        &mut board,
        &mut connections,
        &mut waypoints,
        &mut routes,
        &mut ids,
        full_span,
    );
    couple_diff_pairs(
        &mut board,
        &connections,
        &mut routes,
        &mut ids,
        full_span,
        &paired,
    );
    Strategy {
        board,
        connections,
        routes,
        paired,
    }
}

/// One complete routing: the board as routed, the connections it routed
/// (the requested ones, plus links of nets whose reroutable copper rip-up
/// took apart), and each one's route if it has one.
struct Strategy {
    board: Board,
    connections: Vec<Connection>,
    routes: Vec<Option<OwnRoute>>,
    /// Nets of diff pairs routed side by side ([`route_pairs_together`]).
    paired: Vec<NetId>,
}

/// Routes every two-pin diff pair whose ends sit together as one
/// ([`pair::plan_pair`]), fully checked; returns the nets it routed.
fn route_pairs_together(
    board: &mut Board,
    connections: &[Connection],
    routes: &mut [Option<OwnRoute>],
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
) -> Vec<NetId> {
    let mut paired = Vec::new();
    for (i, lead) in connections.iter().enumerate() {
        let Some((j, partner)) = pair::partner_of(board, connections, lead) else {
            continue;
        };
        if routes[i].is_some() || routes[j].is_some() {
            continue;
        }
        for plan in pair::plan_pair(board, lead, &partner).into_iter().flatten() {
            if !tessera_detail::path_is_legal(board, lead.net, &plan.lead) {
                continue;
            }
            let snapshot = (board.clone(), ids.clone());
            let lead_route = commit(board, lead.net, &plan.lead, ids, full_span);
            if lead_route.is_some()
                && tessera_detail::path_is_legal(board, partner.net, &plan.partner)
            {
                let partner_route = commit(board, partner.net, &plan.partner, ids, full_span);
                if partner_route.is_some() {
                    routes[i] = lead_route;
                    routes[j] = partner_route;
                    paired.extend([lead.net, partner.net]);
                    break;
                }
            }
            (*board, *ids) = snapshot;
        }
    }
    paired
}

/// How many optimisation sweeps at most (it stops once one improves nothing).
const OPTIMIZE_PASSES: usize = 3;
/// The least length saving worth a reroute with the same via count.
const MIN_LENGTH_SAVING_NM: i64 = 100_000;

/// The via count and total track length of one own route.
fn route_metrics(board: &Board, route: &OwnRoute) -> (usize, i64) {
    let length = board
        .tracks
        .iter()
        .filter(|t| route.tracks.contains(&t.id))
        .map(|t| t.segment.length_nm())
        .sum();
    (route.vias.len(), length)
}

/// Rip-up-and-reroute for quality, once routing is done: each route this
/// run made is taken out and routed again against everything else as it
/// finally stands (no corridor — the coarse plan was made for a different
/// board), and the new route kept only if it has fewer vias, or as many and
/// is shorter by `MIN_LENGTH_SAVING_NM`; otherwise the original goes back
/// unchanged. Legal by construction (an ordinary route against everything
/// else), deterministic (connection order), never worse. The second member
/// of a diff pair is left alone, keeping its coupling.
fn optimize(
    board: &mut Board,
    connections: &[Connection],
    routes: &mut [Option<OwnRoute>],
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
    paired: &[NetId],
) {
    let pair_follower = |net: NetId| board_pair_follower(board, connections, net);
    let followers: Vec<bool> = connections
        .iter()
        .map(|c| pair_follower(c.net) || paired.contains(&c.net))
        .collect();
    for _ in 0..OPTIMIZE_PASSES {
        let mut improved = false;
        for i in 0..connections.len() {
            if followers[i] {
                continue;
            }
            let Some(old) = routes[i].take() else {
                continue;
            };
            let (old_vias, old_length) = route_metrics(board, &old);
            let old_tracks: Vec<Track> = board
                .tracks
                .iter()
                .filter(|t| old.tracks.contains(&t.id))
                .cloned()
                .collect();
            let old_via_items: Vec<Via> = board
                .vias
                .iter()
                .filter(|v| old.vias.contains(&v.id))
                .cloned()
                .collect();
            ripup::remove(board, &old);
            let candidate = route_and_commit(board, &connections[i], &[], ids, full_span);
            let better = candidate.as_ref().is_some_and(|new| {
                let (vias, length) = route_metrics(board, new);
                vias < old_vias || (vias == old_vias && length + MIN_LENGTH_SAVING_NM <= old_length)
            });
            if better {
                routes[i] = candidate;
                improved = true;
            } else {
                if let Some(new) = &candidate {
                    ripup::remove(board, new);
                }
                board.tracks.extend(old_tracks);
                board.vias.extend(old_via_items);
                routes[i] = Some(old);
            }
        }
        if !improved {
            break;
        }
    }
}

/// True iff `net` is the second member (the one re-routed beside its
/// partner) of a pair `couple_diff_pairs` handles.
fn board_pair_follower(board: &Board, connections: &[Connection], net: NetId) -> bool {
    let count_for = |n: NetId| connections.iter().filter(|c| c.net == n).count();
    board
        .diff_pair_partner(net)
        .is_some_and(|partner| partner < net && count_for(net) == 1 && count_for(partner) == 1)
}

/// Re-routes the second member of every routed two-pin differential pair
/// beside the first: with a `Coupling` cost that is free at exactly track
/// width + the class's diff-pair gap (centre to centre) from the first
/// member's tracks. An ordinary, fully checked route either way — the
/// coupling only steers it; if it fails, the original route is put back.
fn couple_diff_pairs(
    board: &mut Board,
    connections: &[Connection],
    routes: &mut [Option<OwnRoute>],
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
    paired: &[NetId],
) {
    let count_for = |net: NetId| connections.iter().filter(|c| c.net == net).count();
    for second in 0..connections.len() {
        let net = connections[second].net;
        if paired.contains(&net) {
            continue; // already side by side
        }
        let Some(partner) = board.diff_pair_partner(net) else {
            continue;
        };
        // The lower net id leads; each net must be a single connection.
        if partner > net || count_for(net) != 1 || count_for(partner) != 1 {
            continue;
        }
        let Some(first) = connections.iter().position(|c| c.net == partner) else {
            continue;
        };
        let (Some(lead), Some(_)) = (routes[first].as_ref(), routes[second].as_ref()) else {
            continue;
        };
        let (Some(class), Some(width_nm)) = (board.net_class_for(net), board.track_width_for(net))
        else {
            continue;
        };
        let gap_nm = class
            .diff_pair_gap_nm
            .unwrap_or(class.clearance_nm)
            .max(class.clearance_nm);
        let lead_tracks: Vec<_> = board
            .tracks
            .iter()
            .filter(|t| lead.tracks.contains(&t.id))
            .map(|t| (t.segment, t.layer))
            .collect();
        let coupling =
            tessera_detail::Coupling::new(lead_tracks, board.signal_layers(), width_nm + gap_nm);

        let snapshot = (board.clone(), routes[second].clone(), ids.clone());
        if let Some(old) = routes[second].take() {
            ripup::remove(board, &old);
        }
        routes[second] = tessera_detail::route_connection_with_cost(
            board,
            &connections[second],
            &[],
            Some(&coupling),
        )
        .and_then(|path| commit(board, net, &path, ids, full_span));
        if routes[second].is_none() {
            let (old_board, old_route, old_ids) = snapshot;
            *board = old_board;
            routes[second] = old_route;
            *ids = old_ids;
        }
    }
}

/// Id counters past everything on `board` — and never below `floor`, so an
/// id of copper rip-up took out isn't reused.
fn ids_after(board: &Board, floor: &Ids) -> Ids {
    Ids {
        next_track: board
            .tracks
            .iter()
            .map(|t| t.id.0)
            .max()
            .map_or(0, |m| m + 1)
            .max(floor.next_track),
        next_via: board
            .vias
            .iter()
            .map(|v| v.id.0)
            .max()
            .map_or(0, |m| m + 1)
            .max(floor.next_via),
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
    let width_nm = board.track_width_for(net)?;
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
    for neck in &routed.necks {
        own.tracks.push(TrackId(ids.next_track));
        board.tracks.push(Track {
            id: TrackId(ids.next_track),
            segment: neck.segment,
            width_nm: neck.width_nm,
            layer: neck.layer,
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
/// Pads of a pour net get the same, unless they already sit on the pour
/// (see [`Board::pad_reaches_own_zone`]).
fn pads_needing_fanout(board: &Board) -> Vec<Pad> {
    let signal_layers = board.signal_layers();
    board
        .pads
        .iter()
        .filter(|p| {
            (board.is_plane_net(p.net) && p.layers.iter().all(|l| signal_layers.contains(l)))
                || (board.is_pour_net(p.net) && !board.pad_reaches_own_zone(p))
        })
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
    let mut failed = Vec::new();
    for pad in pads {
        match tessera_detail::fanout_to_plane(board, pad) {
            Some(routed) if commit(board, pad.net, &routed, ids, full_span).is_some() => {
                report.fanouts += 1;
            }
            _ => failed.push(pad),
        }
    }
    // No room for a via beside the pad: a track to the nearest copper of
    // its net that reaches the plane (another pad's fanout via, a
    // through-hole pad) instead — after every pad that could have its own
    // via has it, so there's the most to join.
    for pad in failed {
        if board.is_plane_net(pad.net) && join_plane_elsewhere(board, pad, ids, full_span) {
            report.fanouts += 1;
        } else {
            report.fanout_failed.push(pad.id);
        }
    }
}

/// How many of the nearest plane-reaching items of its net a pad without
/// a fanout via tries to join, and how far away at most.
const PLANE_JOIN_TRIES: usize = 4;
const PLANE_JOIN_REACH_NM: i64 = 10_000_000;

fn dist_sq(a: Point, b: Point) -> i128 {
    let (dx, dy) = (i128::from(a.x - b.x), i128::from(a.y - b.y));
    dx * dx + dy * dy
}

/// Routes `pad` to the nearest via or through-hole pad of its (plane) net
/// that routes: an ordinary, fully checked route.
fn join_plane_elsewhere(
    board: &mut Board,
    pad: &Pad,
    ids: &mut Ids,
    full_span: (LayerId, LayerId),
) -> bool {
    let at = pad.shape.center();
    let signal_layers = board.signal_layers();
    let mut targets: Vec<Point> = board
        .vias
        .iter()
        .filter(|v| v.net == pad.net)
        .map(|v| v.position)
        .chain(
            board
                .pads
                .iter()
                .filter(|p| p.net == pad.net && p.drill_nm.is_some())
                .map(|p| p.shape.center()),
        )
        .filter(|&p| dist_sq(p, at) <= i128::from(PLANE_JOIN_REACH_NM).pow(2))
        .collect();
    targets.sort_by_key(|&p| dist_sq(p, at));
    let layers: Vec<LayerId> = pad
        .layers
        .iter()
        .copied()
        .filter(|l| signal_layers.contains(l))
        .collect();
    for target in targets.into_iter().take(PLANE_JOIN_TRIES) {
        let connection = Connection {
            net: pad.net,
            from: Endpoint {
                position: at,
                layers: layers.clone(),
            },
            to: Endpoint {
                position: target,
                layers: signal_layers.clone(),
            },
        };
        if let Some(path) = tessera_detail::route_connection(board, &connection, &[]) {
            if commit(board, pad.net, &path, ids, full_span).is_some() {
                return true;
            }
        }
    }
    false
}

fn has_own_copper(board: &Board, pad: &Pad) -> bool {
    let touches = |p: Point| match &pad.shape {
        PadShape::Circle(c) => !c.clears_point(p, 1),
        PadShape::Rect(r) => !r.clears_point(p, 1),
        PadShape::Oval(o) => !o.clears_point(p, 1),
        PadShape::Polygon { polygon, .. } => !polygon.clears_point(p, 1),
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

    /// A connection committed as a pointless detour through B.Cu (two
    /// vias) is replaced by the direct route — and only because it's better.
    #[test]
    fn optimize_replaces_a_needless_detour_and_keeps_good_routes() {
        let mut board = two_layer_board();
        let net = NetId(1);
        board.nets.insert(
            net,
            Net {
                id: net,
                name: "A".to_string(),
                net_class: "Default".to_string(),
            },
        );
        let (a, b) = (Point::new(0, 0), Point::new(4_000_000, 0));
        for (id, p) in [(0, a), (1, b)] {
            board.pads.push(Pad {
                id: PadId(id),
                shape: PadShape::Circle(Circle::new(p, 300_000)),
                layers: vec![LayerId(0)],
                net,
                locked: false,
                reference: None,
                number: None,
                drill_nm: None,
            });
        }
        let connection = board.find_unrouted_connections().connections[0].clone();
        let (v1, v2) = (Point::new(1_000_000, 0), Point::new(3_000_000, 0));
        let detour = tessera_detail::RoutedPath {
            segments: vec![
                (Segment::new(a, v1), LayerId(0)),
                (Segment::new(v1, v2), LayerId(1)),
                (Segment::new(v2, b), LayerId(0)),
            ],
            vias: vec![v1, v2],
            necks: vec![],
        };
        let mut ids = Ids {
            next_track: 0,
            next_via: 0,
        };
        let span = (LayerId(0), LayerId(1));
        let mut routes = vec![commit(&mut board, net, &detour, &mut ids, span)];
        assert_eq!(board.vias.len(), 2);

        optimize(
            &mut board,
            std::slice::from_ref(&connection),
            &mut routes,
            &mut ids,
            span,
            &[],
        );
        assert!(board.vias.is_empty());
        assert!(board.tracks.iter().all(|t| t.layer == LayerId(0)));
        assert!(routes[0].is_some());

        // Already optimal: a second run changes nothing.
        let before = board.tracks.clone();
        optimize(
            &mut board,
            std::slice::from_ref(&connection),
            &mut routes,
            &mut ids,
            span,
            &[],
        );
        assert_eq!(board.tracks, before);
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

    fn square(cx: i64, cy: i64, half: i64) -> tessera_geom::Polygon {
        tessera_geom::Polygon::new(vec![
            Point::new(cx - half, cy - half),
            Point::new(cx + half, cy - half),
            Point::new(cx + half, cy + half),
            Point::new(cx - half, cy + half),
        ])
    }

    /// A pad's F.Cu pour island sits over the B.Cu pour of the other end's
    /// island: one via where they overlap joins them, no track needed.
    #[test]
    fn stitches_overlapping_pour_islands_with_a_via() {
        let mut board = two_layer_board();
        let gnd = NetId(1);
        board.nets.insert(
            gnd,
            Net {
                id: gnd,
                name: "GND".to_string(),
                net_class: "Default".to_string(),
            },
        );
        let pad = |id: u32, at: Point, layer: LayerId| Pad {
            id: PadId(id),
            shape: PadShape::Circle(Circle::new(at, 300_000)),
            layers: vec![layer],
            net: gnd,
            locked: false,
            reference: None,
            number: None,
            drill_nm: None,
        };
        board.pads.push(pad(0, Point::new(0, 0), LayerId(0)));
        board
            .pads
            .push(pad(1, Point::new(8_000_000, 0), LayerId(1)));
        let fill = |layer: LayerId, polygon| tessera_model::ZoneFill {
            net: gnd,
            layer,
            clearance_nm: 0,
            polygon,
            inflation_nm: 0,
        };
        board
            .zone_fills
            .push(fill(LayerId(0), square(0, 0, 2_000_000)));
        board
            .zone_fills
            .push(fill(LayerId(1), square(4_000_000, 0, 5_000_000)));
        let connection = Connection {
            net: gnd,
            from: tessera_model::Endpoint {
                position: Point::new(0, 0),
                layers: vec![LayerId(0)],
            },
            to: tessera_model::Endpoint {
                position: Point::new(8_000_000, 0),
                layers: vec![LayerId(1)],
            },
        };
        let path = overlap_stitch(&board, &connection).expect("a stitching via");
        assert!(path.segments.is_empty());
        assert_eq!(path.vias.len(), 1);
        let v = path.vias[0];
        assert!(v.x.abs() < 2_000_000 && v.y.abs() < 2_000_000, "{v:?}");
        // committed, the two islands are one
        let mut ids = Ids {
            next_track: 0,
            next_via: 0,
        };
        commit(&mut board, gnd, &path, &mut ids, (LayerId(0), LayerId(1)));
        assert_eq!(
            tessera_model::connectivity::pad_islands(&board, gnd).len(),
            1
        );
    }
}
