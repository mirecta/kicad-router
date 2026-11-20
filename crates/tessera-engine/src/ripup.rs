//! Progressive rip-up and reroute (plan §5.4), the scheduler's only way to
//! remove copper. It can only ever remove an [`OwnRoute`] — a record of the
//! tracks and vias *this run* committed for one connection — so geometry
//! that was already on the board (the user's own routing, locked or not)
//! can't be a rip-up candidate by construction (plan §7.5.4) — except
//! what the caller explicitly marks reroutable (`Board::reroutable_tracks`,
//! `Board::reroutable_vias`: the router's own output from an earlier pass),
//! which is ripped up per net ([`reroutable_blockers`], [`release`]).

use tessera_detail::{CostField, RoutedPath, CELL_NM};
use tessera_geom::{Circle, Point};
use tessera_model::{Board, Connection, Endpoint, NetId, TrackId, ViaId};

/// The tracks and vias this run committed for one connection.
#[derive(Debug, Clone, Default)]
pub(crate) struct OwnRoute {
    pub(crate) tracks: Vec<TrackId>,
    pub(crate) vias: Vec<ViaId>,
}

/// How many times to sweep over the still-failed connections.
pub(crate) const MAX_PASSES: usize = 4;
/// The most routes one rip-up attempt may remove.
pub(crate) const MAX_BLOCKERS: usize = 6;

pub(crate) fn remove(board: &mut Board, route: &OwnRoute) {
    board.tracks.retain(|t| !route.tracks.contains(&t.id));
    board.vias.retain(|v| !route.vias.contains(&v.id));
}

/// True iff `route`'s copper (on `route_net`) would violate clearance or
/// hole-to-hole spacing against `ghost`, a path planned for `ghost_net` as
/// if `route` weren't there. Deliberately generous (any rule that could
/// make the two collide counts), since a false "blocker" only costs an
/// extra reroute that rip-up rolls back anyway if it doesn't pay off.
pub(crate) fn conflicts(
    board: &Board,
    route: &OwnRoute,
    route_net: NetId,
    ghost: &RoutedPath,
    ghost_net: NetId,
) -> bool {
    let Some(clearance_nm) = board.resolved_clearance_nm(route_net, ghost_net) else {
        return false; // same net
    };
    let (Some(ghost_class), Some((ghost_via_nm, ghost_drill_nm))) = (
        board.net_class_for(ghost_net),
        board.via_size_for(ghost_net),
    ) else {
        return true;
    };
    let ghost_half_width = board
        .track_width_for(ghost_net)
        .unwrap_or(ghost_class.track_width_nm)
        / 2;
    let ghost_vias: Vec<Circle> = ghost
        .vias
        .iter()
        .map(|&p| Circle::new(p, ghost_via_nm / 2))
        .collect();
    let hole_to_hole = board.rules.min_hole_to_hole_nm;

    let tracks = board.tracks.iter().filter(|t| route.tracks.contains(&t.id));
    for t in tracks {
        let half = t.width_nm / 2;
        for (s, layer) in &ghost.segments {
            if *layer == t.layer
                && !t
                    .segment
                    .clears_segment(*s, clearance_nm + half + ghost_half_width)
            {
                return true;
            }
        }
        if ghost_vias
            .iter()
            .any(|v| !v.clears_segment(t.segment, clearance_nm + half))
        {
            return true;
        }
    }
    let vias = board.vias.iter().filter(|v| route.vias.contains(&v.id));
    for v in vias {
        let copper = Circle::new(v.position, v.diameter_nm / 2);
        let drill = Circle::new(v.position, v.drill_nm / 2);
        if ghost
            .segments
            .iter()
            .any(|(s, _)| !copper.clears_segment(*s, clearance_nm + ghost_half_width))
        {
            return true;
        }
        for g in &ghost_vias {
            let g_drill = Circle::new(g.center, ghost_drill_nm / 2);
            if !copper.clears_circle(*g, clearance_nm)
                || !drill.clears_circle(g_drill, hole_to_hole)
            {
                return true;
            }
        }
    }
    false
}

/// A board with every [`OwnRoute`] in `routes` removed — the ghost board a
/// failed connection is planned on to find out what blocks it.
pub(crate) fn without_own_routes(board: &Board, routes: &[Option<OwnRoute>]) -> Board {
    let mut ghost = board.clone();
    for route in routes.iter().flatten() {
        remove(&mut ghost, route);
    }
    remove(&mut ghost, &reroutable(board, None));
    ghost
}

/// The reroutable copper on `board` (of `net`, or of every net), as one
/// route — never of a plane or pour net, whose fanouts reach a zone the
/// router can't see the connectivity of.
fn reroutable(board: &Board, net: Option<NetId>) -> OwnRoute {
    let keep = |n: NetId| {
        net.is_none_or(|net| net == n) && !board.is_plane_net(n) && !board.is_pour_net(n)
    };
    OwnRoute {
        tracks: board
            .tracks
            .iter()
            .filter(|t| board.reroutable_tracks.contains(&t.id) && keep(t.net))
            .map(|t| t.id)
            .collect(),
        vias: board
            .vias
            .iter()
            .filter(|v| board.reroutable_vias.contains(&v.id) && keep(v.net))
            .map(|v| v.id)
            .collect(),
    }
}

/// The reroutable copper in `ghost`'s way, grouped per net (in net order).
pub(crate) fn reroutable_blockers(
    board: &Board,
    ghost: &RoutedPath,
    ghost_net: NetId,
) -> Vec<(NetId, OwnRoute)> {
    let all = reroutable(board, None);
    let mut by_net: Vec<(NetId, OwnRoute)> = Vec::new();
    let mut add = |net: NetId, item: OwnRoute| {
        if net == ghost_net {
            return;
        }
        let i = by_net
            .iter()
            .position(|(n, _)| *n == net)
            .unwrap_or_else(|| {
                by_net.push((net, OwnRoute::default()));
                by_net.len() - 1
            });
        by_net[i].1.tracks.extend(item.tracks);
        by_net[i].1.vias.extend(item.vias);
    };
    for t in board.tracks.iter().filter(|t| all.tracks.contains(&t.id)) {
        let item = OwnRoute {
            tracks: vec![t.id],
            vias: Vec::new(),
        };
        if conflicts(board, &item, t.net, ghost, ghost_net) {
            add(t.net, item);
        }
    }
    for v in board.vias.iter().filter(|v| all.vias.contains(&v.id)) {
        let item = OwnRoute {
            tracks: Vec::new(),
            vias: vec![v.id],
        };
        if conflicts(board, &item, v.net, ghost, ghost_net) {
            add(v.net, item);
        }
    }
    by_net.sort_by_key(|(n, _)| *n);
    by_net
}

/// Rips `items` (reroutable copper of `net`) out of `board`, along with
/// whatever reroutable copper of `net` that leaves dangling, and returns
/// the links that join `net` back up.
pub(crate) fn release(board: &mut Board, net: NetId, items: &OwnRoute) -> Vec<Connection> {
    remove(board, items);
    loop {
        let dangling = dangling(board, net);
        if dangling.tracks.is_empty() && dangling.vias.is_empty() {
            break;
        }
        remove(board, &dangling);
    }
    let islands = tessera_model::connectivity::pad_islands(board, net);
    tessera_model::connectivity::links_between(&islands)
        .into_iter()
        .map(|(from, to): (Endpoint, Endpoint)| Connection { net, from, to })
        .collect()
}

/// Reroutable copper of `net` with a loose end: a track end touching no
/// other copper of the net, or a via joined to fewer than two things.
fn dangling(board: &Board, net: NetId) -> OwnRoute {
    let mine = reroutable(board, Some(net));
    let tracks: Vec<_> = board.tracks.iter().filter(|t| t.net == net).collect();
    let vias: Vec<_> = board.vias.iter().filter(|v| v.net == net).collect();
    let pads: Vec<_> = board.pads.iter().filter(|p| p.net == net).collect();
    let on_pad = |p: Point, layer: tessera_model::LayerId| {
        pads.iter()
            .any(|pad| pad.layers.contains(&layer) && pad_contains(&pad.shape, p))
    };
    let mut out = OwnRoute::default();
    for t in tracks.iter().filter(|t| mine.tracks.contains(&t.id)) {
        let joined = |p: Point| {
            on_pad(p, t.layer)
                || vias
                    .iter()
                    .any(|v| !Circle::new(v.position, v.diameter_nm / 2).clears_point(p, 1))
                // KiCad joins tracks at their ends only: one ending on the
                // middle of another still dangles
                || tracks.iter().any(|o| {
                    o.id != t.id
                        && o.layer == t.layer
                        && [o.segment.a, o.segment.b].iter().any(|&e| {
                            !tessera_geom::Segment::new(e, e).clears_point(p, o.width_nm / 2)
                        })
                })
        };
        if !joined(t.segment.a) || !joined(t.segment.b) {
            out.tracks.push(t.id);
        }
    }
    for v in vias.iter().filter(|v| mine.vias.contains(&v.id)) {
        let at = |p: Point| !Circle::new(v.position, v.diameter_nm / 2).clears_point(p, 1);
        // the layers it actually joins: KiCad calls a via joined on fewer
        // than two dangling
        let mut layers: Vec<tessera_model::LayerId> = tracks
            .iter()
            .filter(|t| at(t.segment.a) || at(t.segment.b))
            .map(|t| t.layer)
            .chain(
                pads.iter()
                    .filter(|pad| pad_contains(&pad.shape, v.position))
                    .flat_map(|pad| pad.layers.iter().copied()),
            )
            .collect();
        layers.sort_unstable();
        layers.dedup();
        if layers.len() < 2 {
            out.vias.push(v.id);
        }
    }
    out
}

fn pad_contains(shape: &tessera_model::PadShape, p: Point) -> bool {
    use tessera_model::PadShape;
    match shape {
        PadShape::Circle(c) => !c.clears_point(p, 1),
        PadShape::Rect(r) => !r.clears_point(p, 1),
        PadShape::Oval(o) => !o.clears_point(p, 1),
        PadShape::Polygon { polygon, .. } => polygon.contains_point(p),
    }
}

/// Extra cost per 0.1 mm step taken through (or within clearance of) one
/// of this run's own routes while planning a ghost: a crossing of a
/// typical track band (~6 cells) then costs about as much as an 18 mm detour,
/// so the plan goes around routes where it reasonably can and crosses as
/// few as it must — the fewer blockers, the likelier rip-up succeeds.
// Measured on the user's esp32-plc with GND/+3.3V planes: plain ghost
// (no cost) 2 nets failed with 8 and 13 blockers; 8000 with the corridor
// changed nothing; 30000 without the corridor routed one of the two.
const OWN_COPPER_COST: i64 = 30_000;

/// Where this run's own routes are, as a cost: the ghost of a failed
/// connection may run through them (they'll be ripped up) but pays for
/// it. Without it the ghost takes the shortest line through the densest
/// spot and collides with a dozen routes — too many to rip up at once.
pub(crate) struct OwnCopper {
    origin: Point,
    width: i64,
    height: i64,
    /// `[layer][y * width + x]`, per signal layer (`Board::signal_layers`).
    cells: Vec<Vec<bool>>,
}

impl OwnCopper {
    /// `margin_nm` is how far from own copper a ghost's centreline still
    /// collides (a track half-width plus clearance is a good estimate).
    pub(crate) fn new(board: &Board, routes: &[Option<OwnRoute>], margin_nm: i64) -> Self {
        let layers = board.signal_layers();
        let earlier = reroutable(board, None);
        let own_tracks: Vec<_> = board
            .tracks
            .iter()
            .filter(|t| {
                earlier.tracks.contains(&t.id)
                    || routes.iter().flatten().any(|r| r.tracks.contains(&t.id))
            })
            .collect();
        let own_vias: Vec<_> = board
            .vias
            .iter()
            .filter(|v| {
                earlier.vias.contains(&v.id)
                    || routes.iter().flatten().any(|r| r.vias.contains(&v.id))
            })
            .collect();
        let points = own_tracks
            .iter()
            .flat_map(|t| [t.segment.a, t.segment.b])
            .chain(own_vias.iter().map(|v| v.position));
        let mut bbox: Option<(i64, i64, i64, i64)> = None;
        for p in points {
            let (x0, y0, x1, y1) = bbox.unwrap_or((p.x, p.y, p.x, p.y));
            bbox = Some((x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)));
        }
        let Some((min_x, min_y, max_x, max_y)) = bbox else {
            return Self {
                origin: Point::new(0, 0),
                width: 0,
                height: 0,
                cells: Vec::new(),
            };
        };
        let (min, max) = (Point::new(min_x, min_y), Point::new(max_x, max_y));
        let pad = margin_nm + 2_000_000;
        let origin = Point::new(min.x - pad, min.y - pad);
        let width = (max.x - min.x + 2 * pad) / CELL_NM + 1;
        let height = (max.y - min.y + 2 * pad) / CELL_NM + 1;
        let n = usize::try_from(width * height).unwrap_or(0);
        let mut cells = vec![vec![false; n]; layers.len()];
        let mut mark = |layer: usize, lo: Point, hi: Point, hit: &dyn Fn(Point) -> bool| {
            let (x0, y0) = ((lo.x - origin.x) / CELL_NM, (lo.y - origin.y) / CELL_NM);
            let (x1, y1) = (
                (hi.x - origin.x) / CELL_NM + 1,
                (hi.y - origin.y) / CELL_NM + 1,
            );
            for y in y0.max(0)..=y1.min(height - 1) {
                for x in x0.max(0)..=x1.min(width - 1) {
                    let p = Point::new(origin.x + x * CELL_NM, origin.y + y * CELL_NM);
                    if hit(p) {
                        if let Ok(i) = usize::try_from(y * width + x) {
                            cells[layer][i] = true;
                        }
                    }
                }
            }
        };
        for t in &own_tracks {
            let Some(layer) = layers.iter().position(|&l| l == t.layer) else {
                continue;
            };
            let r = t.width_nm / 2 + margin_nm;
            let lo = Point::new(
                t.segment.a.x.min(t.segment.b.x) - r,
                t.segment.a.y.min(t.segment.b.y) - r,
            );
            let hi = Point::new(
                t.segment.a.x.max(t.segment.b.x) + r,
                t.segment.a.y.max(t.segment.b.y) + r,
            );
            let seg = t.segment;
            mark(layer, lo, hi, &|p| !seg.clears_point(p, r));
        }
        for v in &own_vias {
            let r = v.diameter_nm / 2 + margin_nm;
            let c = Circle::new(v.position, r);
            let lo = Point::new(v.position.x - r, v.position.y - r);
            let hi = Point::new(v.position.x + r, v.position.y + r);
            for layer in 0..layers.len() {
                mark(layer, lo, hi, &|p| !c.clears_point(p, 0));
            }
        }
        Self {
            origin,
            width,
            height,
            cells,
        }
    }
}

impl CostField for OwnCopper {
    fn cost(&self, p: Point, layer_index: usize) -> i64 {
        let (x, y) = (
            (p.x - self.origin.x) / CELL_NM,
            (p.y - self.origin.y) / CELL_NM,
        );
        if p.x < self.origin.x || p.y < self.origin.y || x >= self.width || y >= self.height {
            return 0;
        }
        let hit = usize::try_from(y * self.width + x)
            .ok()
            .and_then(|i| self.cells.get(layer_index)?.get(i).copied())
            .unwrap_or(false);
        if hit {
            OWN_COPPER_COST
        } else {
            0
        }
    }
}

/// Indices of the connections whose own routes block `ghost`.
pub(crate) fn blockers(
    board: &Board,
    connections: &[Connection],
    routes: &[Option<OwnRoute>],
    ghost: &RoutedPath,
    ghost_net: NetId,
) -> Vec<usize> {
    routes
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let r = r.as_ref()?;
            conflicts(board, r, connections[i].net, ghost, ghost_net).then_some(i)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::Segment;
    use tessera_model::{Layer, LayerId, Track};

    #[test]
    fn own_copper_costs_only_near_own_tracks_on_their_layer() {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
        board.layers.push(Layer::copper(LayerId(1), "B.Cu"));
        let track = |id: u32, y: i64| Track {
            id: TrackId(id),
            segment: Segment::new(Point::new(0, y), Point::new(10_000_000, y)),
            width_nm: 200_000,
            layer: LayerId(0),
            net: NetId(1),
            locked: false,
        };
        board.tracks.push(track(0, 0)); // this run's route
        board.tracks.push(track(1, 5_000_000)); // the user's own
        let routes = vec![Some(OwnRoute {
            tracks: vec![TrackId(0)],
            vias: vec![],
        })];
        let own = OwnCopper::new(&board, &routes, 300_000);
        assert_eq!(own.cost(Point::new(5_000_000, 0), 0), OWN_COPPER_COST);
        assert_eq!(own.cost(Point::new(5_000_000, 300_000), 0), OWN_COPPER_COST);
        assert_eq!(own.cost(Point::new(5_000_000, 1_000_000), 0), 0);
        assert_eq!(own.cost(Point::new(5_000_000, 0), 1), 0);
        assert_eq!(own.cost(Point::new(5_000_000, 5_000_000), 0), 0);
        assert_eq!(own.cost(Point::new(-50_000_000, 0), 0), 0);
    }

    #[test]
    fn no_own_routes_cost_nothing() {
        let own = OwnCopper::new(&Board::new(), &[], 300_000);
        assert_eq!(own.cost(Point::new(0, 0), 0), 0);
    }
}
