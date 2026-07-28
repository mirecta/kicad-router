use tessera_model::{Board, LayerId, NetId, Track, TrackId, Via, ViaId};

#[derive(Debug, Clone, Default)]
pub struct RouteReport {
    pub routed: usize,
    /// Nets a connection existed for but `tessera-detail` couldn't find a
    /// path for within its local search window — left unrouted, not
    /// silently dropped.
    pub failed: Vec<NetId>,
    /// Notes from `Board::find_unrouted_connections` on nets this pass
    /// couldn't even attempt (multi-pin nets — M3 scope).
    pub skipped: Vec<String>,
}

/// Routes every unrouted two-pin net on `board`, committing each
/// successful route as new `Track`/`Via` items directly into `board` so
/// later connections see earlier ones as obstacles.
///
/// This is the M2 baseline: sequential, single-pass, no rip-up/reroute
/// (that scheduling is M5). Connection order is whatever
/// `Board::find_unrouted_connections` returns (currently `HashMap`
/// iteration order — arbitrary, and **not yet deterministic**; plan §5.4
/// requires determinism once parallel rip-up scheduling exists, but a
/// single-threaded M2 baseline with no scheduling has nothing to be
/// nondeterministic *about* yet beyond this ordering. Fix before M5, not
/// before M2).
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
        let Some(net_class) = board.net_class_for(connection.net) else {
            report.failed.push(connection.net);
            continue;
        };
        let width_nm = net_class.track_width_nm;
        let via_diameter_nm = net_class.via_diameter_nm;
        let via_drill_nm = net_class.via_drill_nm;

        let Some(routed) = tessera_detail::route_connection(board, connection) else {
            report.failed.push(connection.net);
            continue;
        };

        for (segment, layer) in &routed.segments {
            board.tracks.push(Track {
                id: TrackId(next_track_id),
                segment: *segment,
                width_nm,
                layer: *layer,
                net: connection.net,
                locked: false,
            });
            next_track_id += 1;
        }
        for position in &routed.vias {
            board.vias.push(Via {
                id: ViaId(next_via_id),
                position: *position,
                diameter_nm: via_diameter_nm,
                drill_nm: via_drill_nm,
                from_layer: full_span.0,
                to_layer: full_span.1,
                net: connection.net,
                locked: false,
            });
            next_via_id += 1;
        }
        report.routed += 1;
    }

    report
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
