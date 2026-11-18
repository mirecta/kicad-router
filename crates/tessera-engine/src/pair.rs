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
/// How far out of its pad, in exit lengths, a track may run before it
/// meets its place in the pair.
const MAX_EXIT_STRETCH: f64 = 1.5;
/// The length difference left between a pair's two tracks without tuning,
/// unless a `skew` rule says otherwise.
const MAX_SKEW_NM: i64 = 100_000;
/// How many of the shorter track's longest straights are tried for the
/// tuning bumps.
const TUNE_SITES: usize = 3;

/// The two routes of a pair: `lead`'s and its partner's.
pub(crate) struct PairRoutes {
    pub(crate) lead: RoutedPath,
    pub(crate) partner: RoutedPath,
    /// Which side of the centreline the lead runs on (+1 left of travel).
    lead_side: i64,
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
                // length-matched first, as routed if no tuning fits
                let max_skew = board
                    .overrides
                    .max_skew_nm
                    .get(&lead.net)
                    .copied()
                    .unwrap_or(MAX_SKEW_NM);
                plans.extend(tuned(&plan, width, max_skew));
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
        lead_side: sign,
    })
}

/// `plan` length-matched: the shorter track given small 45° bumps, away
/// from its partner, on one of its longest straights — what a pair's
/// corners and its exits from the pads leave between the two. One
/// candidate per straight tried (none if the two already match), for the
/// caller to check against the board.
fn tuned(plan: &PairRoutes, width: i64, max_skew: i64) -> Vec<PairRoutes> {
    let (lead_len, partner_len) = (path_length(&plan.lead), path_length(&plan.partner));
    let skew = (lead_len - partner_len).abs();
    if skew <= max_skew {
        return Vec::new();
    }
    let lead_short = lead_len < partner_len;
    let (short, side) = if lead_short {
        (&plan.lead, plan.lead_side)
    } else {
        (&plan.partner, -plan.lead_side)
    };
    // Near where the skew comes from: the end whose two pads are farther
    // apart (the pair fanning out to them), the bumps at that end of each
    // straight; the longest straight, bumps centred, last. Not the stubs
    // out of the pads.
    let other = if lead_short {
        &plan.partner
    } else {
        &plan.lead
    };
    let ends = |p: &RoutedPath| {
        (
            p.segments.first().map(|(s, _)| s.a),
            p.segments.last().map(|(s, _)| s.b),
        )
    };
    let ((Some(a0), Some(a1)), (Some(b0), Some(b1))) = (ends(short), ends(other)) else {
        return Vec::new();
    };
    let at_start = dist(a0, b0) >= dist(a1, b1);
    let n = short.segments.len();
    let mut near: Vec<usize> = (1..n.saturating_sub(1)).collect();
    if !at_start {
        near.reverse();
    }
    let place = if at_start { Place::Start } else { Place::End };
    let longest = near
        .iter()
        .copied()
        .max_by_key(|&i| short.segments[i].0.length_nm());
    let mut paths: Vec<RoutedPath> = near
        .iter()
        .filter_map(|&i| with_bumps(short, i, side, skew, width, place))
        .take(TUNE_SITES)
        .collect();
    paths.extend(longest.and_then(|i| with_bumps(short, i, side, skew, width, Place::Middle)));
    paths
        .into_iter()
        .map(|bumped| {
            let (lead, partner) = if lead_short {
                (bumped, plan.partner.clone())
            } else {
                (plan.lead.clone(), bumped)
            };
            PairRoutes {
                lead,
                partner,
                lead_side: plan.lead_side,
            }
        })
        .collect()
}

/// Where along a straight its tuning bumps go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Start,
    Middle,
    End,
}

/// `path` with segment `i` given as many 45° trapezoid bumps to `side`
/// (+1 left of travel) as add `extra` length, at `place` on it; `None`
/// if they don't fit along it.
fn with_bumps(
    path: &RoutedPath,
    i: usize,
    side: i64,
    extra: i64,
    width: i64,
    place: Place,
) -> Option<RoutedPath> {
    // a 45° trapezoid of height h adds 2h(√2 - 1)
    let gain = 2.0 * (std::f64::consts::SQRT_2 - 1.0);
    let max_height = (3 * width).max(400_000) as f64;
    let count = usize::try_from((extra as f64 / (gain * max_height)).ceil() as i64)
        .unwrap_or(0)
        .max(1);
    let bumps = count as f64;
    let h = extra as f64 / (gain * bumps);
    let (top, space, margin) = (width as f64, width as f64, 2.0 * width as f64);
    let (seg, layer) = path.segments[i];
    let len = seg.length_nm() as f64;
    let footprint = bumps * (2.0 * h + top) + (bumps - 1.0) * space;
    if footprint + 2.0 * margin > len {
        return None;
    }
    let (ux, uy) = (
        (seg.b.x - seg.a.x) as f64 / len,
        (seg.b.y - seg.a.y) as f64 / len,
    );
    let s = side as f64;
    let (nx, ny) = (-uy * s, ux * s);
    let at = |along: f64, up: f64| {
        Point::new(
            seg.a.x + (ux * along + nx * up).round() as i64,
            seg.a.y + (uy * along + ny * up).round() as i64,
        )
    };
    let mut pts = vec![seg.a];
    let mut along = match place {
        Place::Start => margin,
        Place::Middle => (len - footprint) / 2.0,
        Place::End => len - footprint - margin,
    };
    for _ in 0..count {
        pts.extend([
            at(along, 0.0),
            at(along + h, h),
            at(along + h + top, h),
            at(along + 2.0 * h + top, 0.0),
        ]);
        along += 2.0 * h + top + space;
    }
    pts.push(seg.b);
    let mut segments = path.segments[..i].to_vec();
    segments.extend(pts.windows(2).map(|w| (Segment::new(w[0], w[1]), layer)));
    segments.extend_from_slice(&path.segments[i + 1..]);
    Some(RoutedPath {
        segments,
        vias: path.vias.clone(),
        necks: path.necks.clone(),
    })
}

fn path_length(path: &RoutedPath) -> i64 {
    path.segments.iter().map(|(s, _)| s.length_nm()).sum()
}

/// One track of the pair: each run offset by `sign` × the track spread,
/// each via moved the via spread across the run it ends, joined by short
/// jogs, from the pad at `c.from` to the pad at `c.to`.
fn one_side(
    runs: &[(LayerId, Vec<Point>)],
    vias: &[Point],
    conn: &Connection,
    sign: i64,
    spread: Spread,
) -> RoutedPath {
    let mut segments = Vec::new();
    let mut own_vias = Vec::new();
    let mut prev = conn.from.position;
    for (k, (layer, pts)) in runs.iter().enumerate() {
        let mut line = offset_polyline(pts, sign * spread.track);
        // Each track goes out of its own pad, square to the pair's two
        // pads as the pair leaves them, until it meets its offset line —
        // never via the point between the pads (there may be a pin there,
        // an ESD array's ground) nor round the pair's line to its far side
        // (the pads may be well apart, the pair close).
        let exit = |from: Point, to: Point| (to.x - from.x, to.y - from.y);
        if k == 0 {
            line.reverse();
            into_pad(&mut line, conn.from.position, exit(pts[0], pts[1]));
            line.reverse();
        } else {
            lead_from(&mut line, prev); // on from its own via
        }
        let n = pts.len();
        if k + 1 == runs.len() {
            into_pad(&mut line, conn.to.position, exit(pts[n - 1], pts[n - 2]));
            // a via right at the far exit: from it straight to the pad
            let stub = k > 0 && (1..n - 1).all(|i| dist(pts[i], pts[0]) < MIN_VIA_APPROACH_NM);
            if stub {
                line = vec![prev, conn.to.position];
            }
        } else {
            // the via pair, across the route where it changes layers: on
            // the mitre of its turn there (as the pair's corners), from the
            // last real stretch in (past the lead-in from between the pads,
            // not a tiny jog just before the via) to the first one out. If
            // a side is no more than a stub to the pads, square to the
            // other; out of a stub, each track straight from pad to via.
            let first_real = usize::from(k == 0);
            let back = (first_real..n - 1)
                .rev()
                .find(|&i| dist(pts[i], vias[k]) >= MIN_VIA_APPROACH_NM)
                .map(|i| pts[i]);
            let next = &runs[k + 1].1;
            let last_run = k + 2 == runs.len();
            let real = next.len() - usize::from(last_run);
            let ahead = (1..real)
                .find(|&i| dist(next[i], vias[k]) >= MIN_VIA_APPROACH_NM)
                .map(|i| next[i]);
            let d = sign * spread.via;
            let via = match (back, ahead) {
                (Some(b), Some(a)) => offset_polyline(&[b, vias[k], a], d)[1],
                (Some(b), None) => offset_polyline(&[b, vias[k]], d)[1],
                (None, a) => {
                    // the pads are right behind: a little on from them
                    let a = a.unwrap_or(next[next.len() - 1]);
                    line = vec![line[0]];
                    let len = dist(vias[k], a).max(1) as f64;
                    let push = (spread.via / 2).min(dist(vias[k], a) / 2) as f64;
                    let on = Point::new(
                        vias[k].x + ((a.x - vias[k].x) as f64 * push / len).round() as i64,
                        vias[k].y + ((a.y - vias[k].y) as f64 * push / len).round() as i64,
                    );
                    offset_polyline(&[on, a], d)[0]
                }
            };
            // into its own via, not the centre's offset
            if line.len() > 1 {
                line.reverse();
                lead_from(&mut line, via);
                line.reverse();
            } else {
                line.push(via);
            }
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

/// The end of `line` (an offset run whose last point stands for the point
/// between the pads) taken into `pad` instead: cut where it meets the ray
/// out of `pad` along `exit` (the pair's way out of its pads; the last
/// real stretch may be extended to meet it), then along that into the pad.
/// If it doesn't meet it near the pad (the ray runs along the pair, the
/// pads inline with it): as far out of the pad as `exit` goes, then in.
fn into_pad(line: &mut Vec<Point>, pad: Point, exit: (i64, i64)) {
    let cross = |u: (f64, f64), v: (f64, f64)| u.0 * v.1 - u.1 * v.0;
    let dir = (exit.0 as f64, exit.1 as f64);
    let count = line.len();
    for j in (0..count.saturating_sub(2)).rev() {
        let (from, to) = (line[j], line[j + 1]);
        let step = ((to.x - from.x) as f64, (to.y - from.y) as f64);
        let denom = cross(step, dir);
        if denom.abs() < 1e-6 {
            continue;
        }
        let to_pad = ((pad.x - from.x) as f64, (pad.y - from.y) as f64);
        // from + along·step = pad + out·dir
        let (along, out) = (cross(to_pad, dir) / denom, cross(to_pad, step) / denom);
        let last_real = j + 3 == count;
        if along >= 0.0 && (along <= 1.0 || last_real) && (0.0..=MAX_EXIT_STRETCH).contains(&out) {
            let meet = Point::new(
                from.x + (step.0 * along).round() as i64,
                from.y + (step.1 * along).round() as i64,
            );
            line.truncate(j + 1);
            line.extend([meet, pad]);
            line.dedup();
            return;
        }
    }
    if let Some(last) = line.last_mut() {
        *last = Point::new(pad.x + exit.0, pad.y + exit.1);
    }
    line.push(pad);
}

/// `line` (an offset run) led from `via` instead of from its first point:
/// onto its first stretch past the via's foot on it by as much as the via
/// is off it (45°), so the track neither doubles back nor runs along the
/// via without joining it.
fn lead_from(line: &mut Vec<Point>, via: Point) {
    let (a, b) = (line[0], line[1]);
    let len = dist(a, b) as f64;
    if len < 1.0 {
        line[0] = via;
        return;
    }
    let (ux, uy) = ((b.x - a.x) as f64 / len, (b.y - a.y) as f64 / len);
    let (vx, vy) = ((via.x - a.x) as f64, (via.y - a.y) as f64);
    let along = vx * ux + vy * uy;
    let off = (vx * uy - vy * ux).abs();
    let s = (along.max(0.0) + off).min(len);
    line[0] = Point::new(a.x + (ux * s).round() as i64, a.y + (uy * s).round() as i64);
    line.insert(0, via);
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

/// The partner connection of `lead`'s among `connections`: of the pair's
/// other net, the one that runs between the same two places (each end
/// within a pad pitch or so of `lead`'s) — so a pair through an ESD array
/// or a connector, several connections a net, pairs up link by link.
/// (index, the connection oriented like `lead`); the lower net id leads.
pub(crate) fn partner_of(
    board: &Board,
    connections: &[Connection],
    lead: &Connection,
) -> Option<(usize, Connection)> {
    let partner_net: NetId = board.diff_pair_partner(lead.net)?;
    if partner_net < lead.net {
        return None;
    }
    connections
        .iter()
        .enumerate()
        .filter(|(_, c)| c.net == partner_net)
        .filter_map(|(j, c)| {
            let mut p = c.clone();
            let straight =
                dist(lead.from.position, p.from.position) + dist(lead.to.position, p.to.position);
            let swapped =
                dist(lead.from.position, p.to.position) + dist(lead.to.position, p.from.position);
            if swapped < straight {
                std::mem::swap(&mut p.from, &mut p.to);
            }
            let near = dist(lead.from.position, p.from.position) <= MAX_PAIR_SPREAD_NM
                && dist(lead.to.position, p.to.position) <= MAX_PAIR_SPREAD_NM;
            near.then_some((straight.min(swapped), j, p))
        })
        .min_by_key(|(d, j, _)| (*d, *j))
        .map(|(_, j, p)| (j, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bumps_make_up_the_length_difference_on_the_far_side() {
        let l = LayerId(0);
        let straight = |pts: &[Point]| RoutedPath {
            segments: pts
                .windows(2)
                .map(|w| (Segment::new(w[0], w[1]), l))
                .collect(),
            vias: Vec::new(),
            necks: Vec::new(),
        };
        // lead along y = 0, partner along y = 400 µm (to the lead's left,
        // +y), the partner 1 mm longer
        let lead = straight(&[
            Point::new(0, 0),
            Point::new(1_000_000, 0),
            Point::new(11_000_000, 0),
            Point::new(12_000_000, 0),
        ]);
        let partner = straight(&[
            Point::new(0, 400_000),
            Point::new(1_000_000, 400_000),
            Point::new(12_000_000, 400_000),
            Point::new(13_000_000, 400_000),
        ]);
        let plan = PairRoutes {
            lead,
            partner,
            lead_side: -1,
        };
        let tuned = tuned(&plan, 200_000, MAX_SKEW_NM);
        assert!(!tuned.is_empty());
        let t = &tuned[0];
        let skew = path_length(&t.lead) - path_length(&t.partner);
        assert!(skew.abs() <= 1_000, "{skew}");
        // the bumps go away from the partner (to -y)
        assert!(t
            .lead
            .segments
            .iter()
            .all(|(s, _)| s.a.y <= 0 && s.b.y <= 0));
        // at the end the pads fan out to, where the skew comes from
        let bumped: Vec<i64> = t
            .lead
            .segments
            .iter()
            .filter(|(s, _)| s.a.y < 0)
            .map(|(s, _)| s.a.x)
            .collect();
        assert!(
            !bumped.is_empty() && bumped.iter().all(|&x| x > 6_000_000),
            "{bumped:?}"
        );
    }

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
