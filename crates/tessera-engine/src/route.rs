use tessera_model::{Board, Connection, LayerId, NetId, Track, TrackId, Via, ViaId};

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
/// This is the M2/M3 baseline: sequential, single-pass, no rip-up/reroute
/// (that scheduling is M5). Connection order is whatever
/// `Board::find_unrouted_connections` returns (currently `HashMap`
/// iteration order — arbitrary, and **not yet deterministic**; plan §5.4
/// requires determinism once parallel rip-up scheduling exists, but a
/// single-threaded baseline with no scheduling has nothing to be
/// nondeterministic *about* yet beyond this ordering. Fix before M5).
#[must_use]
pub fn route_board(board: &mut Board) -> RouteReport {
    let mut report = RouteReport::default();
    let connection_report = board.find_unrouted_connections();
    report.skipped = connection_report.skipped;

    let mut next_track_id = board
        .tracks
        .iter()
        .map(|t| t.id.0)
        .max()
        .map_or(0, |m| m + 1);
    let mut next_via_id = board.vias.iter().map(|v| v.id.0).max().map_or(0, |m| m + 1);
    let full_span = full_layer_span(board);

    for connection in &connection_report.connections {
        if route_and_commit(
            board,
            connection,
            &mut next_track_id,
            &mut next_via_id,
            full_span,
        ) {
            report.routed += 1;
        } else {
            report.failed.push(connection.net);
        }
    }

    for (net, endpoints) in &connection_report.multi_pin_nets {
        let edges = tessera_global::minimum_spanning_tree(endpoints);
        let mut every_edge_routed = !edges.is_empty();
        for edge in &edges {
            let connection = Connection {
                net: *net,
                from: edge.from.clone(),
                to: edge.to.clone(),
            };
            if !route_and_commit(
                board,
                &connection,
                &mut next_track_id,
                &mut next_via_id,
                full_span,
            ) {
                every_edge_routed = false;
            }
        }
        if every_edge_routed {
            report.routed += 1;
        } else {
            report.failed.push(*net);
        }
    }

    report
}

/// Routes one two-pin `connection` via `tessera-detail` and, on success,
/// commits the result into `board` as new `Track`/`Via` items. Returns
/// whether it succeeded.
fn route_and_commit(
    board: &mut Board,
    connection: &Connection,
    next_track_id: &mut u32,
    next_via_id: &mut u32,
    full_span: (LayerId, LayerId),
) -> bool {
    let Some(net_class) = board.net_class_for(connection.net) else {
        return false;
    };
    let width_nm = net_class.track_width_nm;
    let via_diameter_nm = net_class.via_diameter_nm;
    let via_drill_nm = net_class.via_drill_nm;

    let Some(routed) = tessera_detail::route_connection(board, connection) else {
        return false;
    };

    for (segment, layer) in &routed.segments {
        board.tracks.push(Track {
            id: TrackId(*next_track_id),
            segment: *segment,
            width_nm,
            layer: *layer,
            net: connection.net,
            locked: false,
        });
        *next_track_id += 1;
    }
    for position in &routed.vias {
        board.vias.push(Via {
            id: ViaId(*next_via_id),
            position: *position,
            diameter_nm: via_diameter_nm,
            drill_nm: via_drill_nm,
            from_layer: full_span.0,
            to_layer: full_span.1,
            net: connection.net,
            locked: false,
        });
        *next_via_id += 1;
    }
    true
}

/// M2 scope: every via is a through via spanning the board's full copper
/// stack (blind/buried vias are plan §7.1, not yet modelled).
fn full_layer_span(board: &Board) -> (LayerId, LayerId) {
    let mut ids: Vec<LayerId> = board.layers.iter().map(|l| l.id).collect();
    ids.sort_by_key(|l| l.0);
    match (ids.first(), ids.last()) {
        (Some(&first), Some(&last)) => (first, last),
        _ => (LayerId(0), LayerId(0)),
    }
}
