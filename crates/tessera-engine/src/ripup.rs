//! Progressive rip-up and reroute (plan §5.4), the scheduler's only way to
//! remove copper. It can only ever remove an [`OwnRoute`] — a record of the
//! tracks and vias *this run* committed for one connection — so geometry
//! that was already on the board (the user's own routing, locked or not)
//! can't be a rip-up candidate by construction (plan §7.5.4).

use tessera_detail::RoutedPath;
use tessera_geom::Circle;
use tessera_model::{Board, Connection, NetId, TrackId, ViaId};

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
    let ghost_half_width = ghost_class.track_width_nm / 2;
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
    ghost
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
