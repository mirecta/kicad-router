//! The user's stitching vias — free vias tying a pour to a plane or to
//! the same pour on another layer — are lifted while routing and put back
//! afterwards, at their own spot if it's still free, else at the nearest
//! free spot inside the same pour on every layer they joined. Hand-placed
//! scattered vias otherwise sit right where new tracks need to pass.

use tessera_geom::{Circle, Point};
use tessera_model::{Board, NetId, PadShape, Via, ViaId};

/// How far from its own spot a lifted via may be put back.
const MAX_MOVE_NM: i64 = 3_000_000;
/// Grid of candidate spots around it.
const STEP_NM: i64 = 100_000;

/// A via taken off the board, and the pours it joined.
pub struct Lifted {
    via: Via,
    /// Fills (indices into `Board::zone_fills`) of its net it sat on.
    fills: Vec<usize>,
    /// Copper zones (indices into `Board::copper_zones`) of its net whose
    /// outline it sat inside.
    zones: Vec<usize>,
}

/// Takes the user's stitching vias off `board` (none if
/// `Board::fixed_vias`): unlocked vias of a net with a copper zone or a
/// plane, with no track of their net touching them and no pad under them
/// (a via in a pad is a thermal or a fanout, not a stitch), whose spot is
/// legal as the board stands.
pub fn lift(board: &mut Board) -> Vec<Lifted> {
    if board.fixed_vias {
        return Vec::new();
    }
    let candidates: Vec<Via> = board
        .vias
        .iter()
        .filter(|v| is_stitching(board, v))
        .cloned()
        .collect();
    // One whose own spot breaks a rule already (hole-to-hole to a
    // neighbour, say) stays put: the user accepted it there, and it would
    // only be moved for that, not for the routing.
    let stitching: Vec<ViaId> = {
        let mut without = board.clone();
        without
            .vias
            .retain(|v| !candidates.iter().any(|c| c.id == v.id));
        let check = tessera_detail::ViaCheck::new(&without);
        candidates
            .iter()
            .filter(|v| check.fits(v.net, v.position, v.diameter_nm, v.drill_nm))
            .map(|v| v.id)
            .collect()
    };
    let mut lifted = Vec::new();
    for id in stitching {
        let Some(at) = board.vias.iter().position(|v| v.id == id) else {
            continue;
        };
        let via = board.vias.remove(at);
        let fills = board
            .zone_fills
            .iter()
            .enumerate()
            .filter(|(_, f)| f.net == via.net && f.polygon.contains_point(via.position))
            .map(|(k, _)| k)
            .collect();
        let zones = board
            .copper_zones
            .iter()
            .enumerate()
            .filter(|(_, z)| {
                z.net == via.net
                    && z.outline
                        .as_ref()
                        .is_some_and(|o| o.contains_point(via.position))
            })
            .map(|(k, _)| k)
            .collect();
        lifted.push(Lifted { via, fills, zones });
    }
    lifted
}

fn is_stitching(board: &Board, via: &Via) -> bool {
    let body = Circle::new(via.position, via.diameter_nm / 2);
    let poured = board.is_plane_net(via.net) || board.copper_zones.iter().any(|z| z.net == via.net);
    let wired = board
        .tracks
        .iter()
        .any(|t| t.net == via.net && !body.clears_segment(t.segment, t.width_nm / 2));
    let in_pad = board.pads.iter().any(|p| !pad_clears(&p.shape, body));
    !via.locked && poured && !wired && !in_pad
}

fn pad_clears(shape: &PadShape, body: Circle) -> bool {
    match shape {
        PadShape::Circle(c) => c.clears_circle(body, 0),
        PadShape::Rect(r) => r.clears_circle(body, 0),
        PadShape::Oval(o) => o.clears_circle(body, 0),
        PadShape::Polygon { polygon, .. } => polygon.clears_circle(body, 0),
    }
}

/// What became of the lifted vias.
#[derive(Debug, Default)]
pub struct Placed {
    /// Net, old spot, new spot.
    pub moved: Vec<(NetId, Point, Point)>,
    /// Net and spot of those no free spot was found for nearby — the pour
    /// island stitching after this joins what they joined, if needed.
    pub dropped: Vec<(NetId, Point)>,
}

/// Puts `lifted` back on `board`, one at a time against everything
/// placed so far: unchanged where its spot is still legal, else moved to
/// the nearest legal spot within [`MAX_MOVE_NM`] that is still inside every
/// fill and zone outline of its net it was in (with its whole pad on each
/// fill), else dropped. A moved via gets a new id from `next_via`.
pub fn place_back(board: &mut Board, lifted: Vec<Lifted>, next_via: &mut u32) -> Placed {
    let mut placed = Placed::default();
    let mut offsets: Vec<(i64, i64)> = Vec::new();
    let reach = MAX_MOVE_NM / STEP_NM;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            if dx * dx + dy * dy <= reach * reach {
                offsets.push((dx * STEP_NM, dy * STEP_NM));
            }
        }
    }
    offsets.sort_by_key(|&(dx, dy)| (dx * dx + dy * dy, dy, dx));
    for Lifted { via, fills, zones } in lifted {
        let spot = {
            let check = tessera_detail::ViaCheck::new(board);
            let radius = via.diameter_nm / 2;
            let inside = |p: Point| {
                fills.iter().all(|&k| {
                    let polygon = &board.zone_fills[k].polygon;
                    [(0, 0), (radius, 0), (-radius, 0), (0, radius), (0, -radius)]
                        .iter()
                        .all(|&(dx, dy)| polygon.contains_point(Point::new(p.x + dx, p.y + dy)))
                }) && zones.iter().all(|&k| {
                    board.copper_zones[k]
                        .outline
                        .as_ref()
                        .is_some_and(|o| o.contains_point(p))
                })
            };
            offsets
                .iter()
                .map(|&(dx, dy)| Point::new(via.position.x + dx, via.position.y + dy))
                .find(|&p| {
                    (p == via.position || inside(p))
                        && check.fits(via.net, p, via.diameter_nm, via.drill_nm)
                })
        };
        match spot {
            Some(p) if p == via.position => board.vias.push(via),
            Some(p) => {
                placed.moved.push((via.net, via.position, p));
                board.vias.push(Via {
                    id: ViaId(*next_via),
                    position: p,
                    ..via
                });
                *next_via += 1;
            }
            None => placed.dropped.push((via.net, via.position)),
        }
    }
    placed
}
