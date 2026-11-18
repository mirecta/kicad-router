//! Differential pairs routed as one: the pair's centreline is routed as a
//! single fat track (both tracks and the gap between them, as if one net),
//! then split into the two tracks either side of it — so they run side by
//! side at the class's diff-pair gap the whole way, the way a designer
//! routes USB. A fat via on the way becomes the pair's two vias side by
//! side across the route. Whatever doesn't fit falls back to routing the
//! two separately and coupling the second to the first
//! (`couple_diff_pairs`).

// Board coordinates are bounded far below 2^52 nm (a metre is 1e9 nm), so
// the f64 round trips for offsets and mitres are exact enough and in range.
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use tessera_detail::RoutedPath;
use tessera_geom::{Point, Segment};
use tessera_model::{Board, Connection, Endpoint, LayerId, NetId};

/// How far apart a pair's two pads may be at either end to be routed as
/// one (a connector's or IC's adjacent pins).
const MAX_PAIR_SPREAD_NM: i64 = 3_000_000;
/// Extra half-width of the fat route over the two tracks and the gap, for
/// the outer track's corners (a mitred offset reaches a little further
/// than the centreline's round-capped one).
const CORNER_MARGIN_NM: i64 = 30_000;
/// How far back along the route a pair via's sideways direction is taken.
const MIN_VIA_APPROACH_NM: i64 = 500_000;
/// Extra spread of a pair's two vias beyond what clearance needs.
const VIA_MARGIN_NM: i64 = 50_000;

/// The two routes of a pair: `lead`'s and its partner's.
pub(crate) struct PairRoutes {
    pub(crate) lead: RoutedPath,
    pub(crate) partner: RoutedPath,
}

/// Plans `lead` and `partner` (connections of a diff pair's two nets,
/// `partner` oriented the same way) as one: candidates, best first, for
/// the caller to check (empty if none).
pub(crate) fn plan_pair(
    board: &Board,
    lead: &Connection,
    partner: &Connection,
) -> Option<Vec<PairRoutes>> {
    let width = board.track_width_for(lead.net)?;
    let class = board.net_class_for(lead.net)?;
    let gap = class
        .diff_pair_gap_nm
        .unwrap_or(class.clearance_nm)
        .max(class.clearance_nm);
    let offset = i64::midpoint(width, gap);
    let fat = 2 * (offset + width / 2 + CORNER_MARGIN_NM);
    let (via_d, drill) = board.via_size_for(lead.net)?;
    let via_gap = class
        .diff_pair_via_gap_nm
        .unwrap_or(class.clearance_nm)
        .max(class.clearance_nm);
    // apart enough for the two vias, and for each one to clear the other
    // track running past it
    let via =
        i64::midpoint(via_d, via_gap).max(via_d / 2 + class.clearance_nm + width / 2 - offset);
    let spread = Spread {
        track: offset,
        via: via + VIA_MARGIN_NM,
    };
    if dist(lead.from.position, partner.from.position) > MAX_PAIR_SPREAD_NM
        || dist(lead.to.position, partner.to.position) > MAX_PAIR_SPREAD_NM
    {
        return None;
    }
    let shared = |a: &Endpoint, b: &Endpoint| -> Vec<LayerId> {
        a.layers
            .iter()
            .copied()
            .filter(|l| b.layers.contains(l))
            .collect()
    };
    let (from_layers, to_layers) = (
        shared(&lead.from, &partner.from),
        shared(&lead.to, &partner.to),
    );
    let layer = *from_layers.iter().find(|l| to_layers.contains(l))?;

    // the centreline, on a board where the pair is one net
    let mut ghost = board.clone();
    for p in &mut ghost.pads {
        if p.net == partner.net {
            p.net = lead.net;
        }
    }
    for t in &mut ghost.tracks {
        if t.net == partner.net {
            t.net = lead.net;
        }
    }
    for v in &mut ghost.vias {
        if v.net == partner.net {
            v.net = lead.net;
        }
    }
    ghost.overrides.track_width_nm.insert(lead.net, fat);
    ghost.overrides.min_track_width_nm.insert(lead.net, fat); // no neck-down
                                                              // one fat via standing for the pair's two, side by side
    ghost
        .overrides
        .via_nm
        .insert(lead.net, (2 * spread.via + via_d, 2 * spread.via + drill));
    // Out of the pads first, square to the line between them, then on:
    // a pair leaving along its own pins would run over one of them.
    let exit_len = width + gap + pad_reach(board, lead, partner);
    let exits = |a: Point, b: Point, toward: Point| -> Vec<Point> {
        let m = mid(a, b);
        let (vx, vy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
        let len = vx.hypot(vy).max(1.0);
        let (px, py) = (-vy / len, vx / len);
        let e = exit_len as f64;
        let one = Point::new(m.x + (px * e).round() as i64, m.y + (py * e).round() as i64);
        let other = Point::new(m.x - (px * e).round() as i64, m.y - (py * e).round() as i64);
        // towards the other end first
        if dist(one, toward) <= dist(other, toward) {
            vec![one, other]
        } else {
            vec![other, one]
        }
    };
    let (mid_from, mid_to) = (
        mid(lead.from.position, partner.from.position),
        mid(lead.to.position, partner.to.position),
    );
    let mut plans = Vec::new();
    for start in exits(lead.from.position, partner.from.position, mid_to) {
        for end in exits(lead.to.position, partner.to.position, mid_from) {
            if let Some(plan) = split(
                &ghost,
                lead,
                partner,
                [mid_from, start, end, mid_to],
                layer,
                spread,
            ) {
                plans.push(plan);
            }
        }
    }
    Some(plans)
}

/// How far the pair's two tracks, and its two vias, sit from the
/// centreline.
#[derive(Debug, Clone, Copy)]
struct Spread {
    track: i64,
    via: i64,
}

/// The pair along a fat route from `start` to `end` (the exits), led in
/// from `mid_from` and out to `mid_to` (between each end's two pads). A
/// fat via on the way becomes two vias side by side across the route.
fn split(
    ghost: &Board,
    lead: &Connection,
    partner: &Connection,
    [mid_from, start, end, mid_to]: [Point; 4],
    layer: LayerId,
    spread: Spread,
) -> Option<PairRoutes> {
    let centre = Connection {
        net: lead.net,
        from: Endpoint {
            position: start,
            layers: vec![layer],
        },
        to: Endpoint {
            position: end,
            layers: vec![layer],
        },
    };
    let path = tessera_detail::route_connection(ghost, &centre, &[])?;
    if !path.necks.is_empty() || path.segments.is_empty() {
        return None;
    }
    // the centreline as runs per layer, led in and out
    let mut runs: Vec<(LayerId, Vec<Point>)> = Vec::new();
    for (seg, l) in &path.segments {
        match runs.last_mut() {
            Some((rl, pts)) if rl == l && pts.last() == Some(&seg.a) => pts.push(seg.b),
            _ => runs.push((*l, vec![seg.a, seg.b])),
        }
    }
    if runs.len() != path.vias.len() + 1 {
        return None;
    }
    runs[0].1.insert(0, mid_from);
    runs.last_mut()?.1.push(mid_to);
    for (_, pts) in &mut runs {
        pts.dedup();
        if pts.len() < 2 {
            return None;
        }
    }
    // which side the lead runs on, at both ends the same
    let side = |pad: Point, at: Point, towards: Point| {
        let (ux, uy) = (towards.x - at.x, towards.y - at.y);
        let cross =
            i128::from(ux) * i128::from(pad.y - at.y) - i128::from(uy) * i128::from(pad.x - at.x);
        cross.signum()
    };
    let (first, last) = (&runs[0].1, &runs.last()?.1);
    let s0 = side(lead.from.position, first[0], first[1]);
    let s1 = side(lead.to.position, last[last.len() - 2], last[last.len() - 1]);
    if s0 == 0 || s0 != s1 {
        return None; // the pair would have to cross over
    }
    let sign = s0 as i64;
    let lead_path = one_side(&runs, &path.vias, lead, sign, spread);
    let partner_path = one_side(&runs, &path.vias, partner, -sign, spread);
    Some(PairRoutes {
        lead: lead_path,
        partner: partner_path,
    })
}

/// One track of the pair: each run offset by `sign` × the track spread,
/// each via moved the via spread across the run it ends, joined by short
/// jogs, from the pad at `c.from` to the pad at `c.to`.
fn one_side(
    runs: &[(LayerId, Vec<Point>)],
    vias: &[Point],
    c: &Connection,
    sign: i64,
    spread: Spread,
) -> RoutedPath {
    let mut segments = Vec::new();
    let mut own_vias = Vec::new();
    let mut prev = c.from.position;
    for (k, (layer, pts)) in runs.iter().enumerate() {
        let mut line = offset_polyline(pts, sign * spread.track);
        // Each track goes from its own pad straight to where the pair
        // starts running together (the exit), not via the point between
        // the pads — there may be a pin there (an ESD array's ground).
        // First straight out of the pad as far as the exit, then across to
        // the pair's line: a pad may be well apart from its partner.
        let out = |pad: Point, from: Point, to: Point| {
            Point::new(pad.x + to.x - from.x, pad.y + to.y - from.y)
        };
        if k == 0 {
            line[0] = out(c.from.position, pts[0], pts[1]);
            line.insert(0, c.from.position);
        } else {
            line.insert(0, prev);
        }
        if k + 1 == runs.len() {
            let n = pts.len();
            if let Some(last) = line.last_mut() {
                *last = out(c.to.position, pts[n - 1], pts[n - 2]);
            }
            line.push(c.to.position);
        } else {
            // the via, across the end of this run — square to its last
            // real stretch, not to a tiny jog just before the via
            let n = pts.len();
            let back = (0..n - 1)
                .rev()
                .find(|&i| dist(pts[i], vias[k]) >= MIN_VIA_APPROACH_NM)
                .unwrap_or(0);
            let via = offset_polyline(&[pts[back], vias[k]], sign * spread.via)[1];
            line.push(via);
            own_vias.push(via);
            prev = via;
        }
        line.dedup();
        let line = without_spikes(line, 2 * spread.track);
        segments.extend(line.windows(2).map(|w| (Segment::new(w[0], w[1]), *layer)));
    }
    RoutedPath {
        segments,
        vias: own_vias,
        necks: Vec::new(),
    }
}

/// The largest half-extent of the pair's four end pads.
fn pad_reach(board: &Board, lead: &Connection, partner: &Connection) -> i64 {
    let ends = [
        lead.from.position,
        lead.to.position,
        partner.from.position,
        partner.to.position,
    ];
    board
        .pads
        .iter()
        .filter(|p| ends.contains(&p.shape.center()))
        .map(|p| {
            let (hx, hy) = p.shape.half_extents();
            hx.max(hy)
        })
        .max()
        .unwrap_or(0)
}

/// `line` moved sideways by `d` (positive: to the left of travel in board
/// coordinates, the side `cross > 0` points to), corners mitred.
fn offset_polyline(line: &[Point], d: i64) -> Vec<Point> {
    let normal = |a: Point, b: Point| {
        let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
        let len = dx.hypot(dy);
        // left of travel: (dx, dy) turned so that cross(u, n) > 0
        (-dy / len, dx / len)
    };
    let d = d as f64;
    let mut out = Vec::with_capacity(line.len());
    for i in 0..line.len() {
        let n = if i == 0 {
            normal(line[0], line[1])
        } else if i == line.len() - 1 {
            normal(line[i - 1], line[i])
        } else {
            let (a, b) = (normal(line[i - 1], line[i]), normal(line[i], line[i + 1]));
            let (sx, sy) = (a.0 + b.0, a.1 + b.1);
            let dot = (sx * a.0 + sy * a.1).max(1e-9);
            (sx / dot, sy / dot) // the mitre: length 1/cos(half the turn)
        };
        out.push(Point::new(
            line[i].x + (n.0 * d).round() as i64,
            line[i].y + (n.1 * d).round() as i64,
        ));
    }
    out
}

/// `line` without the tiny spikes (shorter than `max_nm`) where it doubles
/// back on itself — what offsetting a centreline's small jogs around a via
/// leaves. A long way out and back (a pad behind the exit) stays.
fn without_spikes(mut line: Vec<Point>, max_nm: i64) -> Vec<Point> {
    let mut i = 1;
    while i + 1 < line.len() {
        let (a, b, c) = (line[i - 1], line[i], line[i + 1]);
        let dot = i128::from(b.x - a.x) * i128::from(c.x - b.x)
            + i128::from(b.y - a.y) * i128::from(c.y - b.y);
        if dot < 0 && dist(a, b).min(dist(b, c)) < max_nm {
            line.remove(i);
            i = i.saturating_sub(1).max(1);
        } else {
            i += 1;
        }
    }
    line
}

fn mid(a: Point, b: Point) -> Point {
    Point::new(i64::midpoint(a.x, b.x), i64::midpoint(a.y, b.y))
}

fn dist(a: Point, b: Point) -> i64 {
    ((a.x - b.x) as f64).hypot((a.y - b.y) as f64) as i64
}

/// The partner connection of `net`'s pair among `connections`, if each
/// net is exactly one connection: (index, the connection oriented like
/// `lead`).
pub(crate) fn partner_of(
    board: &Board,
    connections: &[Connection],
    lead: &Connection,
) -> Option<(usize, Connection)> {
    let partner_net: NetId = board.diff_pair_partner(lead.net)?;
    let count = |n: NetId| connections.iter().filter(|c| c.net == n).count();
    if partner_net < lead.net || count(lead.net) != 1 || count(partner_net) != 1 {
        return None;
    }
    let j = connections.iter().position(|c| c.net == partner_net)?;
    let mut p = connections[j].clone();
    let straight =
        dist(lead.from.position, p.from.position) + dist(lead.to.position, p.to.position);
    let swapped = dist(lead.from.position, p.to.position) + dist(lead.to.position, p.from.position);
    if swapped < straight {
        std::mem::swap(&mut p.from, &mut p.to);
    }
    Some((j, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_keep_the_distance_through_a_45_degree_turn() {
        let line = [
            Point::new(0, 0),
            Point::new(5_000_000, 0),
            Point::new(8_000_000, 3_000_000),
        ];
        let left = offset_polyline(&line, 300_000);
        // the side where cross(travel, p) > 0 — with y pointing down, +y
        // for travel along +x
        assert_eq!(left[0], Point::new(0, 300_000));
        let seg = |a: Point, b: Point| Segment::new(a, b);
        // every offset segment runs 0.3 mm from its centreline segment
        for i in 0..2 {
            let (c, o) = (seg(line[i], line[i + 1]), seg(left[i], left[i + 1]));
            let m = Point::new(i64::midpoint(o.a.x, o.b.x), i64::midpoint(o.a.y, o.b.y));
            assert!(!c.clears_point(m, 300_000 + 1_000) && c.clears_point(m, 300_000 - 1_000));
        }
    }
}
