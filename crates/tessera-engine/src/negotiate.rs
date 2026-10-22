//! Negotiated congestion (`PathFinder`, McMurchie & Ebeling) over every
//! connection at once: each round, the nets that collide are re-planned
//! against the board's fixed geometry, with the other nets' current plans
//! as a cost rather than an obstacle; collisions make their cells dearer
//! for the next round, until the plans stop colliding.

use std::collections::BTreeMap;

use tessera_detail::{Congestion, RoutedPath};
use tessera_model::{Board, Connection, NetId};

/// Negotiation rounds at most (it stops early once nothing collides).
const ROUNDS: usize = 15;
/// Cost per other net covering a cell, in step-cost units (a straight grid
/// step is 1000), at the first round…
const PRESENT_START: i64 = 500;
/// …growing by this percentage each round, so collisions get steadily
/// dearer than detours or vias.
const PRESENT_GROWTH_PERCENT: i64 = 30;
/// History added per round to each cell where routes collided.
const HISTORY_STEP: i64 = 1000;

/// A plan per connection (`None` where even the uncontested search found
/// nothing), from the round whose plans collided least — rounds don't
/// necessarily improve monotonically. Plans are searched over the full
/// window without the global router's corridor: a corridor search succeeds
/// whenever nothing *hard* blocks it, which would keep a route inside a
/// congested corridor no matter how dear it gets.
pub(crate) fn plan(board: &Board, connections: &[Connection]) -> Vec<Option<RoutedPath>> {
    let mut planned: Vec<Option<RoutedPath>> = vec![None; connections.len()];
    let Some(mut congestion) = Congestion::new(board, PRESENT_START) else {
        return planned;
    };
    let mut by_net: BTreeMap<NetId, Vec<usize>> = BTreeMap::new();
    for (i, c) in connections.iter().enumerate() {
        by_net.entry(c.net).or_default().push(i);
    }
    let mut best: (usize, Vec<Option<RoutedPath>>) = (usize::MAX, planned.clone());
    for round in 0..ROUNDS {
        for (&net, indices) in &by_net {
            // Only nets that collide (or have an unplanned connection) are
            // re-planned; the rest keep their plans and footprints.
            let settled = round > 0
                && !congestion.collides(net)
                && indices.iter().all(|&i| planned[i].is_some());
            if settled {
                continue;
            }
            congestion.clear_net(net);
            for &i in indices {
                planned[i] = tessera_detail::route_connection_with_cost(
                    board,
                    &connections[i],
                    &[],
                    Some(&congestion),
                );
            }
            let (Some(class), Some(width_nm), Some((via_nm, _))) = (
                board.net_class_for(net),
                board.track_width_for(net),
                board.via_size_for(net),
            ) else {
                continue;
            };
            let paths: Vec<&RoutedPath> = indices
                .iter()
                .filter_map(|&i| planned[i].as_ref())
                .collect();
            congestion.set_net(
                net,
                &paths,
                class.clearance_nm + width_nm,
                class.clearance_nm + via_nm / 2 + width_nm / 2,
            );
        }
        let collisions = congestion.end_round(HISTORY_STEP, PRESENT_GROWTH_PERCENT);
        if collisions < best.0 {
            best = (collisions, planned.clone());
        }
        if collisions == 0 {
            break;
        }
    }
    best.1
}
