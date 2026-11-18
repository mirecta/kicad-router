use tessera_geom::{Circle, Point, Segment};
use tessera_model::{Board, Connection, LayerId, NetId, Pad, PadShape};

use crate::astar::{self, State};
use crate::cost::CostField;
use crate::grid::{Cell, CorridorMask, GridBounds, ObstacleMap, CELL_NM, MOVE_HALF_LENGTH_SQ_NM};
use crate::obstacle::{obstacles_from_board, Blocks, Obstacle};
use crate::smooth::{straighten, ClearanceCheck};

/// A margin, in nanometres, added around a connection's endpoints when
/// sizing the local search grid — enough room to route around a nearby
/// obstacle without ballooning the search space for a short connection.
/// 6mm sits in the middle of a stable plateau measured on a real board
/// (3–8mm swept: 66, 67, 70, 69, 70, 68 nets routed) — single runs are
/// too sensitive to routing-order effects to tune on one value.
const SEARCH_MARGIN_NM: i64 = 6_000_000;

/// Half-width of the hard corridor tube confining a waypoint-guided search
/// (see `route_connection`'s docs) — deliberately narrower than
/// `SEARCH_MARGIN_NM`, so the constraint actually shrinks the search space
/// and shortens paths rather than being a no-op alongside it. Not tuned
/// against a real corpus yet.
const CORRIDOR_HALF_WIDTH_NM: i64 = 1_500_000;

/// One layer segment of a routed path, plus any via placed at its end to
/// change layers. `tessera-engine` (M2's orchestration layer) turns this
/// into actual `Track`/`Via` items to commit back to the board.
#[derive(Debug, Clone)]
pub struct RoutedPath {
    /// Straight segments in path order, each tagged with the layer it's on.
    pub segments: Vec<(Segment, LayerId)>,
    /// Via positions where the path changes layers, in path order.
    pub vias: Vec<Point>,
    /// Narrower stubs out of fine-pitch pads the net's own width doesn't
    /// fit beside (see [`Neck`]); empty for an ordinary route.
    pub necks: Vec<Neck>,
}

/// A short track narrower than its net's width, from a pad's centre to
/// where the full width first fits — how a 0.6 mm supply track leaves a
/// 0.65 mm-pitch TSSOP pin (KiCad's DRC checks only the minimum track
/// width, not the class width, so this is legal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Neck {
    pub segment: Segment,
    pub layer: LayerId,
    pub width_nm: i64,
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
/// coordinates by the caller). When non-empty, two things happen: the
/// search window's bounding box expands to cover every waypoint, not just
/// `connection`'s own endpoints (as before); and the search is first tried
/// hard-confined to a tube (`CorridorMask`, half-width
/// `CORRIDOR_HALF_WIDTH_NM`) around the polyline from `connection.from`
/// through the waypoints to `connection.to` — not just biased toward it.
/// If that constrained search fails (the corridor was too tight for real
/// geometry the coarse global grid didn't know about), this falls back to
/// the unconstrained full-window search rather than reporting failure, so
/// a hint can only ever make a route shorter/more predictable, never less
/// likely to succeed than passing an empty slice would have. Pass an empty
/// slice for the plain start/end-only window with no corridor at all.
#[must_use]
pub fn route_connection(
    board: &Board,
    connection: &Connection,
    waypoints: &[Point],
) -> Option<RoutedPath> {
    route_connection_with_cost(board, connection, waypoints, None)
}

/// [`route_connection`], with an extra per-cell cost (if given) added to
/// every search step — how negotiated congestion steers a route away from
/// other nets, and how a diff pair's second member is kept beside the
/// first, without forbidding anything.
#[must_use]
pub fn route_connection_with_cost(
    board: &Board,
    connection: &Connection,
    waypoints: &[Point],
    cost: Option<&dyn CostField>,
) -> Option<RoutedPath> {
    // The grid is aligned so one endpoint sits exactly on a grid point:
    // between fine-pitch pins (0.5mm pitch, 0.3mm pads) a grid point even
    // 0.05mm off the pad centre can already fall inside a neighbouring
    // pin's clearance, leaving the search nowhere to start. Try the start
    // pad's alignment, then the goal's.
    route_aligned(board, connection, waypoints, connection.from.position, cost)
        .or_else(|| route_aligned(board, connection, waypoints, connection.to.position, cost))
        .or_else(|| route_necked(board, connection, waypoints, cost))
}

/// Extra cost per 0.1 mm step where the track would only graze its own
/// net's copper — crossing that thin band to reach a pad is cheap, running
/// along it isn't.
const GRAZE_COST: i64 = 2_000;

/// Extra cost per 0.1 mm step where only the narrow neck width fits — a
/// necked route runs narrow for as short a stretch as it can.
const NECK_COST: i64 = 3_000;
/// The narrowest neck ever made, whatever the board's minimum allows.
const MIN_NECK_WIDTH_NM: i64 = 100_000;
/// How finely a necked route is cut when widening it back.
const NECK_PIECE_NM: i64 = 200_000;

/// [`route_connection`] for a connection with an endpoint pad too tightly
/// pitched for its net's track width: routed at a width that fits there,
/// dearer wherever only that width fits, then widened back to full width
/// everywhere it's clear — only the stretch that has to stay narrow is a
/// [`Neck`]. `None` if no endpoint needs it, or it routes no better.
fn route_necked(
    board: &Board,
    connection: &Connection,
    waypoints: &[Point],
    cost: Option<&dyn CostField>,
) -> Option<RoutedPath> {
    let width_nm = board.track_width_for(connection.net)?;
    let obstacles = obstacles_from_board(board);
    let full = ClearanceCheck::new(board, &obstacles, connection.net, width_nm / 2);
    let narrow_nm = [&connection.from, &connection.to]
        .into_iter()
        .filter_map(|end| neck_width(board, &obstacles, &full, connection.net, end, width_nm))
        .min()?;
    let widths = Widths {
        half_nm: narrow_nm / 2,
        full_half_nm: Some(width_nm / 2),
    };
    route_aligned_at(
        board,
        connection,
        waypoints,
        connection.from.position,
        cost,
        widths,
    )
    .or_else(|| {
        route_aligned_at(
            board,
            connection,
            waypoints,
            connection.to.position,
            cost,
            widths,
        )
    })
}

/// The width a track may leave `end`'s pad at, if its net's full
/// `width_nm` doesn't fit on the pad centre on any of the pad's signal
/// layers; `None` if it fits (or `end` isn't a pad, or nothing narrower
/// fits either). Candidates, widest first: the pad's own narrow side, the
/// Default class width, the board's minimum.
fn neck_width(
    board: &Board,
    obstacles: &[Obstacle],
    full: &ClearanceCheck<'_>,
    net: NetId,
    end: &tessera_model::Endpoint,
    width_nm: i64,
) -> Option<i64> {
    let c = end.position;
    let signal = board.signal_layers();
    let pad = board.pads.iter().find(|p| {
        p.net == net && p.shape.center() == c && p.layers.iter().any(|l| end.layers.contains(l))
    })?;
    let layers: Vec<LayerId> = end
        .layers
        .iter()
        .copied()
        .filter(|l| signal.contains(l) && pad.layers.contains(l))
        .collect();
    let fits = |check: &ClearanceCheck<'_>| {
        layers
            .iter()
            .any(|&l| check.segment_is_clear(l, Segment::new(c, c)))
    };
    if fits(full) {
        return None;
    }
    let (hx, hy) = pad.shape.half_extents();
    let floor = board.rules.min_track_width_nm.max(MIN_NECK_WIDTH_NM);
    let mut widths: Vec<i64> = [
        2 * hx.min(hy),
        board
            .net_classes
            .get("Default")
            .map_or(0, |d| d.track_width_nm),
        floor,
    ]
    .into_iter()
    .filter(|&w| w < width_nm && w >= floor)
    .collect();
    widths.sort_unstable_by(|a, b| b.cmp(a));
    widths
        .into_iter()
        .find(|&w| fits(&ClearanceCheck::new(board, obstacles, net, w / 2)))
}

/// Splits `path`'s segments, routed at `narrow_nm`, into full-width ones
/// wherever `full` finds them clear and [`Neck`]s elsewhere.
fn widen(path: RoutedPath, full: &ClearanceCheck<'_>, narrow_nm: i64) -> RoutedPath {
    let mut segments = Vec::new();
    let mut necks = Vec::new();
    for (segment, layer) in path.segments {
        let (a, b) = (segment.a, segment.b);
        let n = (segment.length_nm() / NECK_PIECE_NM).max(1);
        let at = |i: i64| Point::new(a.x + (b.x - a.x) * i / n, a.y + (b.y - a.y) * i / n);
        let wide: Vec<bool> = (0..n)
            .map(|i| full.segment_is_clear(layer, Segment::new(at(i), at(i + 1))))
            .collect();
        let mut start = 0;
        for i in 1..=wide.len() {
            if i == wide.len() || wide[i] != wide[start] {
                let piece = Segment::new(
                    at(i64::try_from(start).unwrap_or(0)),
                    at(i64::try_from(i).unwrap_or(n)),
                );
                if wide[start] {
                    segments.push((piece, layer));
                } else {
                    necks.push(Neck {
                        segment: piece,
                        layer,
                        width_nm: narrow_nm,
                    });
                }
                start = i;
            }
        }
    }
    RoutedPath {
        segments,
        vias: path.vias,
        necks,
    }
}

/// [`route_connection`] on a grid with `anchor` exactly on a grid point.
fn route_aligned(
    board: &Board,
    connection: &Connection,
    waypoints: &[Point],
    anchor: Point,
    cost: Option<&dyn CostField>,
) -> Option<RoutedPath> {
    let widths = Widths {
        half_nm: board.track_width_for(connection.net)? / 2,
        full_half_nm: None,
    };
    route_aligned_at(board, connection, waypoints, anchor, cost, widths)
}

/// The per-window soft costs every route pays on top of length and vias:
/// crowding another net's thermal spokes, running where only a neck fits
/// (necked routes), grazing its own net's copper (boards with a minimum
/// connection width).
struct SoftCosts {
    spokes: crate::thermal::SpokeField,
    neck_only: Option<ObstacleMap>,
    grazes: Option<ObstacleMap>,
}

impl SoftCosts {
    fn build(
        board: &Board,
        obstacles: &[Obstacle],
        net: NetId,
        bounds: GridBounds,
        layers: &[LayerId],
        widths: Widths,
    ) -> Self {
        let spokes = crate::thermal::SpokeField::build(
            board,
            net,
            bounds,
            layers,
            widths.half_nm + board.net_class_for(net).map_or(0, |c| c.clearance_nm),
        );
        let neck_only = widths
            .full_half_nm
            .map(|half| track_map(board, obstacles, net, half, bounds, layers));
        let grazes = (board.rules.min_connection_nm > 0).then(|| {
            let mut m = ObstacleMap::build(board, &[], net, 0, 0, bounds, layers);
            m.block_grazes(
                obstacles,
                net,
                widths.half_nm,
                board.rules.min_connection_nm,
                layers,
            );
            m
        });
        Self {
            spokes,
            neck_only,
            grazes,
        }
    }

    fn cost(&self, cell: Cell, layer_index: usize) -> i64 {
        let on = |m: &Option<ObstacleMap>, c: i64| {
            m.as_ref()
                .filter(|m| m.is_blocked(cell, layer_index))
                .map_or(0, |_| c)
        };
        self.spokes.cost(cell, layer_index)
            + on(&self.neck_only, NECK_COST)
            + on(&self.grazes, GRAZE_COST)
    }
}

/// The half-width a route is searched at, and — for a necked route — the
/// net's full half-width it's widened back to.
#[derive(Debug, Clone, Copy)]
struct Widths {
    half_nm: i64,
    full_half_nm: Option<i64>,
}

fn route_aligned_at(
    board: &Board,
    connection: &Connection,
    waypoints: &[Point],
    anchor: Point,
    cost: Option<&dyn CostField>,
    widths: Widths,
) -> Option<RoutedPath> {
    let route_half_width_nm = widths.half_nm;

    let layers = board.signal_layers();

    let xs = [connection.from.position.x, connection.to.position.x]
        .into_iter()
        .chain(waypoints.iter().map(|p| p.x));
    let ys = [connection.from.position.y, connection.to.position.y]
        .into_iter()
        .chain(waypoints.iter().map(|p| p.y));
    let (min_x, max_x) = xs.fold((i64::MAX, i64::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
    let (min_y, max_y) = ys.fold((i64::MAX, i64::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));

    let snap_below =
        |a: i64, limit: i64| a - (a - limit + CELL_NM - 1).div_euclid(CELL_NM) * CELL_NM;
    let origin = Point::new(
        snap_below(anchor.x, min_x - SEARCH_MARGIN_NM),
        snap_below(anchor.y, min_y - SEARCH_MARGIN_NM),
    );
    let far = Point::new(max_x + SEARCH_MARGIN_NM, max_y + SEARCH_MARGIN_NM);
    let width = i32::try_from((far.x - origin.x) / CELL_NM + 1).ok()?;
    let height = i32::try_from((far.y - origin.y) / CELL_NM + 1).ok()?;
    let bounds = GridBounds {
        origin,
        width,
        height,
    };

    let obstacles = obstacles_from_board(board);
    let Maps { map, via_map } = build_maps(
        board,
        &obstacles,
        connection.net,
        route_half_width_nm,
        bounds,
        &layers,
    )?;
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

    let soft = SoftCosts::build(board, &obstacles, connection.net, bounds, &layers, widths);
    let extra = |cell: Cell, layer_index: usize| {
        cost.map_or(0, |c| c.cost(cell.to_point(origin), layer_index))
            + soft.cost(cell, layer_index)
    };
    let check = ClearanceCheck::new(board, &obstacles, connection.net, route_half_width_nm);
    let full_check = widths
        .full_half_nm
        .map(|half| ClearanceCheck::new(board, &obstacles, connection.net, half));
    let finish = |path: Vec<State>| {
        let routed = path_to_routed(&path, origin, &layers, connection, &check, cost);
        match &full_check {
            Some(full) => widen(routed, full, 2 * route_half_width_nm),
            None => routed,
        }
    };

    if waypoints.is_empty() {
        return astar::search(&map, &via_map, &starts, &goals, None, &extra).map(finish);
    }

    let mut polyline = Vec::with_capacity(waypoints.len() + 2);
    polyline.push(connection.from.position);
    polyline.extend_from_slice(waypoints);
    polyline.push(connection.to.position);
    let mut corridor = CorridorMask::build(&polyline, CORRIDOR_HALF_WIDTH_NM, bounds);
    for &(cell, _) in starts.iter().chain(goals.iter()) {
        corridor.mark_inside(cell);
    }

    astar::search(&map, &via_map, &starts, &goals, Some(&corridor), &extra)
        .or_else(|| astar::search(&map, &via_map, &starts, &goals, None, &extra))
        .map(finish)
}

/// Where a track of `net` with `half_width_nm` may go.
fn track_map(
    board: &Board,
    obstacles: &[Obstacle],
    net: NetId,
    half_width_nm: i64,
    bounds: GridBounds,
    layers: &[LayerId],
) -> ObstacleMap {
    let for_tracks: Vec<Obstacle> = obstacles
        .iter()
        .filter(|o| o.blocks != Blocks::ViasOnly)
        .cloned()
        .collect();
    ObstacleMap::build(
        board,
        &for_tracks,
        net,
        half_width_nm,
        MOVE_HALF_LENGTH_SQ_NM,
        bounds,
        layers,
    )
}

struct Maps {
    /// Where a new track of the routed net may go.
    map: ObstacleMap,
    /// Where a new via of the routed net may go (its own diameter, every
    /// layer, and hole-to-hole spacing from every existing drill).
    via_map: ObstacleMap,
}

fn build_maps(
    board: &Board,
    obstacles: &[Obstacle],
    net: NetId,
    half_width_nm: i64,
    bounds: GridBounds,
    layers: &[LayerId],
) -> Option<Maps> {
    let map = track_map(board, obstacles, net, half_width_nm, bounds, layers);
    let for_vias: Vec<Obstacle> = obstacles
        .iter()
        .filter(|o| o.blocks != Blocks::TracksOnly)
        .cloned()
        .collect();
    // A via's footprint is its own diameter, not the track's width, and it
    // occupies every layer it spans — a separate map, checked differently
    // (see astar::search's docs), not the same map reused with a bigger
    // number.
    let (via_diameter_nm, via_drill_nm) = board.via_size_for(net)?;
    let mut via_map = ObstacleMap::build(
        board,
        &for_vias,
        net,
        via_diameter_nm / 2,
        0,
        bounds,
        layers,
    );
    via_map.block_grazes(
        obstacles,
        net,
        via_diameter_nm / 2,
        board.rules.min_connection_nm,
        layers,
    );
    via_map.block_drill_conflicts(
        &drill_circles(board),
        via_drill_nm / 2,
        board.rules.min_hole_to_hole_nm,
    );
    Some(Maps { map, via_map })
}

/// How far (as a path) a fanout may wander from its pad looking for a via
/// site, and the window searched for it.
const FANOUT_REACH_NM: i64 = 2_000_000;

/// Connects one pad of a plane net to its plane: a short track from the pad
/// to the nearest spot where a via fits — outside the pad itself, keeping
/// net-class clearance from its copper, so the via doesn't sit in the
/// solder area — then a through via. `None` if no such spot is within
/// reach. The pad must be on a signal layer; a through-hole pad already
/// reaches every plane through its own hole and needs no fanout.
#[must_use]
pub fn fanout_to_plane(board: &Board, pad: &Pad) -> Option<RoutedPath> {
    // A pour net's via must land where its own zone is, on a layer other
    // than the pad's, or it would dangle.
    let pour_zones: Vec<&tessera_geom::Polygon> = if board.is_pour_net(pad.net) {
        board
            .copper_zones
            .iter()
            .filter(|z| z.net == pad.net && z.layers.iter().any(|l| !pad.layers.contains(l)))
            .filter_map(|z| z.outline.as_ref())
            .collect()
    } else {
        Vec::new()
    };
    let in_pour =
        |p: Point| !board.is_pour_net(pad.net) || pour_zones.iter().any(|z| z.contains_point(p));
    fanout_into(board, pad, &in_pour)
}

/// A spot within `lo`..`hi` where a through via of `net` is legal and
/// `accept` holds (scanned on the 0.1 mm grid, row by row; the first one
/// found). How a stitching via between two overlapping pour islands of one
/// net on different layers finds its place.
#[must_use]
pub fn via_site(
    board: &Board,
    net: NetId,
    lo: Point,
    hi: Point,
    accept: &dyn Fn(Point) -> bool,
) -> Option<Point> {
    let layers = board.signal_layers();
    let origin = Point::new(
        lo.x - lo.x.rem_euclid(CELL_NM),
        lo.y - lo.y.rem_euclid(CELL_NM),
    );
    let bounds = GridBounds {
        origin,
        width: i32::try_from((hi.x - origin.x) / CELL_NM + 1).ok()?,
        height: i32::try_from((hi.y - origin.y) / CELL_NM + 1).ok()?,
    };
    let obstacles = obstacles_from_board(board);
    let half_width_nm = board.track_width_for(net)? / 2;
    let Maps { via_map, .. } = build_maps(board, &obstacles, net, half_width_nm, bounds, &layers)?;
    (0..bounds.height)
        .flat_map(|y| (0..bounds.width).map(move |x| Cell { x, y }))
        .filter(|&c| !via_map.blocked_on_any_layer(c))
        .map(|c| c.to_point(origin))
        .find(|&p| accept(p))
}

/// [`fanout_to_plane`], with the via restricted to where `target` holds —
/// e.g. on a pour fill of the pad's net, to stitch the pad's island of
/// that pour to it.
#[must_use]
pub fn fanout_into(board: &Board, pad: &Pad, target: &dyn Fn(Point) -> bool) -> Option<RoutedPath> {
    let layers = board.signal_layers();
    let layer = *pad.layers.iter().find(|l| layers.contains(l))?;
    let layer_index = layers.iter().position(|&l| l == layer)?;
    let center = pad.shape.center();
    let reach = FANOUT_REACH_NM + CELL_NM;
    let origin = Point::new(center.x - reach, center.y - reach);
    let cells = i32::try_from(2 * reach / CELL_NM + 1).ok()?;
    let bounds = GridBounds {
        origin,
        width: cells,
        height: cells,
    };
    let obstacles = obstacles_from_board(board);
    let half_width_nm = board.track_width_for(pad.net)? / 2;
    let Maps { map, via_map } =
        build_maps(board, &obstacles, pad.net, half_width_nm, bounds, &layers)?;
    let (via_diameter_nm, _) = board.via_size_for(pad.net)?;
    let spacing_nm = board.net_class_for(pad.net)?.clearance_nm;
    let outside_pad = |cell: Cell| {
        let via = Circle::new(cell.to_point(origin), via_diameter_nm / 2);
        match &pad.shape {
            PadShape::Circle(c) => c.clears_circle(via, spacing_nm),
            PadShape::Rect(r) => r.clears_circle(via, spacing_nm),
            PadShape::Oval(o) => o.clears_circle(via, spacing_nm),
            PadShape::Polygon { polygon, .. } => polygon.clears_circle(via, spacing_nm),
        }
    };
    let in_pour = |cell: Cell| target(cell.to_point(origin));
    let start = Cell::from_point(center, origin);
    let max_cost = FANOUT_REACH_NM / CELL_NM * 1000;
    let path = astar::nearest(&map, (start, layer_index), max_cost, |cell| {
        !via_map.blocked_on_any_layer(cell) && outside_pad(cell) && in_pour(cell)
    })?;

    let check = ClearanceCheck::new(board, &obstacles, pad.net, half_width_nm);
    let mut points: Vec<Point> = path.iter().map(|s| s.cell.to_point(origin)).collect();
    if points[0] != center && check.segment_is_clear(layer, Segment::new(center, points[0])) {
        points.insert(0, center);
    }
    let via = *points.last()?;
    let polyline = straighten(&points, layer, &check, None);
    Some(RoutedPath {
        segments: polyline
            .windows(2)
            .map(|w| (Segment::new(w[0], w[1]), layer))
            .collect(),
        vias: vec![via],
        necks: Vec::new(),
    })
}

/// True iff committing `path` for `net` onto `board` would break no rule
/// the router itself routes by: every track segment and via checked
/// exactly against every obstacle (clearance, holes, zones, the board edge,
/// keepouts), plus via hole-to-hole spacing against every existing drill.
/// Negotiated routing plans paths without seeing each other as obstacles;
/// this is what makes committing one safe.
#[must_use]
pub fn path_is_legal(board: &Board, net: NetId, path: &RoutedPath) -> bool {
    let Some(width_nm) = board.track_width_for(net) else {
        return false;
    };
    let Some((via_diameter_nm, via_drill_nm)) = board.via_size_for(net) else {
        return false;
    };
    let obstacles = obstacles_from_board(board);
    let check = ClearanceCheck::new(board, &obstacles, net, width_nm / 2);
    if !path
        .segments
        .iter()
        .all(|(s, layer)| check.segment_is_clear(*layer, *s))
    {
        return false;
    }
    if !path.necks.iter().all(|n| {
        ClearanceCheck::new(board, &obstacles, net, n.width_nm / 2)
            .segment_is_clear(n.layer, n.segment)
    }) {
        return false;
    }
    let drills = drill_circles(board);
    path.vias.iter().all(|&v| {
        let clear_of_obstacles = obstacles
            .iter()
            .filter(|o| o.blocks != Blocks::TracksOnly)
            .all(|o| {
                o.required_clearance_nm(board, net)
                    .is_none_or(|req| o.shape.clears_point(v, req + via_diameter_nm / 2))
            });
        let drill = Circle::new(v, via_drill_nm / 2);
        clear_of_obstacles
            && drills
                .iter()
                .all(|d| d.clears_circle(drill, board.rules.min_hole_to_hole_nm))
    })
}

/// Every drilled hole already on the board: vias, plated pads, and
/// non-plated holes.
fn drill_circles(board: &Board) -> Vec<Circle> {
    let vias = board
        .vias
        .iter()
        .map(|v| Circle::new(v.position, v.drill_nm / 2));
    let pads = board
        .pads
        .iter()
        .filter_map(|p| p.drill_nm.map(|d| Circle::new(p.shape.center(), d / 2)));
    let holes = board.npth_holes.iter().map(tessera_model::Hole::circle);
    vias.chain(pads).chain(holes).collect()
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

/// Turns an A* path into tracks and vias: each same-layer run is
/// straightened (see [`straighten`]), and the route's two ends attach to
/// the actual pad centres rather than the nearest grid points whenever
/// that short stub is itself clear.
fn path_to_routed(
    path: &[State],
    origin: Point,
    layers: &[LayerId],
    connection: &Connection,
    check: &ClearanceCheck<'_>,
    cost: Option<&dyn CostField>,
) -> RoutedPath {
    let mut runs: Vec<&[State]> = Vec::new();
    let mut vias = Vec::new();
    let mut run_start = 0usize;
    for i in 1..path.len() {
        if path[i].layer != path[i - 1].layer {
            runs.push(&path[run_start..i]);
            vias.push(path[i - 1].cell.to_point(origin));
            run_start = i;
        }
    }
    runs.push(&path[run_start..]);

    let last = runs.len() - 1;
    let mut segments = Vec::new();
    for (k, run) in runs.iter().enumerate() {
        let Some(first) = run.first() else { continue };
        let layer = layers[first.layer];
        let mut points: Vec<Point> = run.iter().map(|s| s.cell.to_point(origin)).collect();
        let stub_ok = |pad: Point, grid: Point| {
            pad != grid && check.segment_is_clear(layer, Segment::new(pad, grid))
        };
        if k == 0
            && connection.from.layers.contains(&layer)
            && stub_ok(connection.from.position, points[0])
        {
            points.insert(0, connection.from.position);
        }
        if k == last
            && connection.to.layers.contains(&layer)
            && stub_ok(connection.to.position, points[points.len() - 1])
        {
            points.push(connection.to.position);
        }
        let polyline = straighten(&points, layer, check, cost.map(|c| (c, first.layer)));
        segments.extend(
            polyline
                .windows(2)
                .map(|w| (Segment::new(w[0], w[1]), layer)),
        );
    }

    RoutedPath {
        segments,
        vias,
        necks: Vec::new(),
    }
}
