use tessera_geom::{orient, Orientation, Point, Segment};
use tessera_model::{Board, LayerId, NetId};

use crate::obstacle::{Obstacle, ObstacleShape};

/// How many consecutive failed candidates the straightening scan tolerates
/// before settling for the farthest point it already reached — a later
/// point can be reachable even when an intermediate one isn't.
const LOOKAHEAD: usize = 12;

struct Item<'a> {
    layer: LayerId,
    shape: &'a ObstacleShape,
    required_nm: i64,
    min: Point,
    max: Point,
}

/// An exact "may a new straight track of this net go here" test, using the
/// same `tessera-geom` predicates and per-obstacle clearance resolution as
/// the grid map, but on real segments instead of cell centres.
pub(crate) struct ClearanceCheck<'a> {
    items: Vec<Item<'a>>,
}

impl<'a> ClearanceCheck<'a> {
    pub(crate) fn new(
        board: &Board,
        obstacles: &'a [Obstacle],
        routed_net: NetId,
        half_width_nm: i64,
    ) -> Self {
        let items = obstacles
            .iter()
            .filter_map(|o| {
                let required_nm = o.required_clearance_nm(board, routed_net)? + half_width_nm;
                let (min, max) = o.shape.bounding_box(required_nm);
                Some(Item {
                    layer: o.layer,
                    shape: &o.shape,
                    required_nm,
                    min,
                    max,
                })
            })
            .collect();
        Self { items }
    }

    pub(crate) fn segment_is_clear(&self, layer: LayerId, segment: Segment) -> bool {
        let (lo_x, hi_x) = (segment.a.x.min(segment.b.x), segment.a.x.max(segment.b.x));
        let (lo_y, hi_y) = (segment.a.y.min(segment.b.y), segment.a.y.max(segment.b.y));
        self.items
            .iter()
            .filter(|it| it.layer == layer)
            .filter(|it| {
                it.min.x <= hi_x && lo_x <= it.max.x && it.min.y <= hi_y && lo_y <= it.max.y
            })
            .all(|it| it.shape.clears_segment(segment, it.required_nm))
    }
}

/// Rewrites one same-layer stretch of a grid path as few straight and 45°
/// segments as clearance allows: from each vertex, jump to the farthest
/// later point reachable by one octilinear segment, or by two (a 45° leg
/// and a straight one, in either order). Every segment emitted is checked
/// exactly, so this can only shorten and clean up a route, never make it
/// less clear. Consecutive input points must already be connected by clear
/// segments (true for grid steps, and checked by the caller for a pad-centre
/// stub).
pub(crate) fn straighten(
    points: &[Point],
    layer: LayerId,
    check: &ClearanceCheck<'_>,
) -> Vec<Point> {
    let Some(&first) = points.first() else {
        return Vec::new();
    };
    let mut out = vec![first];
    let mut i = 0;
    while i + 1 < points.len() {
        let mut best = (i + 1, Vec::new());
        let mut misses = 0;
        for (j, &target) in points.iter().enumerate().skip(i + 1) {
            if let Some(corner) = octilinear_route(points[i], target, layer, check) {
                best = (j, corner);
                misses = 0;
            } else {
                misses += 1;
                if misses > LOOKAHEAD {
                    break;
                }
            }
        }
        let (j, corner) = best;
        out.extend(corner);
        out.push(points[j]);
        i = j;
    }
    merge_collinear(out)
}

/// Corner size of the chamfered L the straightener prefers.
const CHAMFER_NM: i64 = 500_000;
/// A pure 45° run longer than this is used only when nothing tidier fits.
const MAX_FREE_DIAGONAL_NM: i64 = 2_000_000;

/// A clear octilinear connection from `a` to `b`, as the corner points in
/// between (none for a single segment), or `None` if nothing fits. Shapes
/// are tried tidiest-first, the way a person routes: a straight run; a
/// short 45° run; an L of horizontal and vertical runs with a 45°
/// chamfered corner (either way round); and only then a long 45° leg.
fn octilinear_route(
    a: Point,
    b: Point,
    layer: LayerId,
    check: &ClearanceCheck<'_>,
) -> Option<Vec<Point>> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let clear = |corners: &[Point]| {
        let mut prev = a;
        corners.iter().chain(std::iter::once(&b)).all(|&p| {
            let ok = check.segment_is_clear(layer, Segment::new(prev, p));
            prev = p;
            ok
        })
    };
    if dx == 0 || dy == 0 {
        return clear(&[]).then(Vec::new);
    }
    let (sx, sy) = (dx.signum(), dy.signum());
    let pure_diagonal = dx.abs() == dy.abs();
    let mut candidates: Vec<Vec<Point>> = Vec::new();
    if pure_diagonal && dx.abs() <= MAX_FREE_DIAGONAL_NM {
        candidates.push(Vec::new());
    }
    let c = CHAMFER_NM.min(dx.abs()).min(dy.abs());
    candidates.push(vec![
        Point::new(b.x - sx * c, a.y),
        Point::new(b.x, a.y + sy * c),
    ]);
    candidates.push(vec![
        Point::new(a.x, b.y - sy * c),
        Point::new(a.x + sx * c, b.y),
    ]);
    if pure_diagonal {
        candidates.push(Vec::new());
    } else {
        let d = dx.abs().min(dy.abs());
        candidates.push(vec![Point::new(a.x + sx * d, a.y + sy * d)]);
        candidates.push(vec![Point::new(b.x - sx * d, b.y - sy * d)]);
    }
    candidates.into_iter().find(|corners| clear(corners))
}

fn merge_collinear(points: Vec<Point>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if out.last() == Some(&p) {
            continue;
        }
        if out.len() >= 2
            && orient(out[out.len() - 2], out[out.len() - 1], p) == Orientation::Collinear
        {
            out.pop();
        }
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_geom::Circle;
    use tessera_model::{Layer, Net, NetClass};

    use crate::obstacle::{ObstacleClearance, ObstacleKind};

    fn board() -> Board {
        let mut board = Board::new();
        board.layers.push(Layer::copper(LayerId(0), "F.Cu"));
        let mut class = NetClass::default_placeholder();
        class.clearance_nm = 200_000;
        board.net_classes.insert("Default".to_string(), class);
        for id in [1, 2] {
            board.nets.insert(
                NetId(id),
                Net {
                    id: NetId(id),
                    name: format!("N{id}"),
                    net_class: "Default".to_string(),
                },
            );
        }
        board
    }

    /// A grid staircase (alternating straight and diagonal steps).
    fn staircase(steps: i64) -> Vec<Point> {
        let mut pts = vec![Point::new(0, 0)];
        let (mut x, mut y) = (0, 0);
        for k in 0..steps {
            x += 100_000;
            if k % 2 == 1 {
                y += 100_000;
            }
            pts.push(Point::new(x, y));
        }
        pts
    }

    #[test]
    fn an_unobstructed_staircase_becomes_a_chamfered_l() {
        let board = board();
        let check = ClearanceCheck::new(&board, &[], NetId(1), 100_000);
        let out = straighten(&staircase(20), LayerId(0), &check);
        assert_eq!(
            out,
            vec![
                Point::new(0, 0),
                Point::new(1_500_000, 0),
                Point::new(2_000_000, 500_000),
                Point::new(2_000_000, 1_000_000),
            ]
        );
    }

    #[test]
    fn a_short_45_degree_run_stays_a_single_segment() {
        let board = board();
        let check = ClearanceCheck::new(&board, &[], NetId(1), 100_000);
        let path: Vec<Point> = (0..=10)
            .map(|i| Point::new(i * 100_000, i * 100_000))
            .collect();
        assert_eq!(
            straighten(&path, LayerId(0), &check),
            vec![path[0], path[10]]
        );
    }

    #[test]
    fn straightening_never_cuts_through_an_obstacle() {
        let board = board();
        // A via-sized obstacle sitting on the straight line from start to end.
        let obstacles = [Obstacle {
            shape: ObstacleShape::Circle(Circle::new(Point::new(1_000_000, 0), 300_000)),
            layer: LayerId(0),
            net: NetId(2),
            kind: ObstacleKind::Movable,
            clearance: ObstacleClearance::NetClass,
        }];
        let check = ClearanceCheck::new(&board, &obstacles, NetId(1), 100_000);
        // A grid path detouring over the obstacle.
        let mut path: Vec<Point> = (0..=4).map(|i| Point::new(i * 100_000, 0)).collect();
        path.extend((1..=7).map(|i| Point::new(400_000 + i * 100_000, -i * 100_000)));
        path.extend((1..=7).map(|i| Point::new(1_100_000 + i * 100_000, -700_000 + i * 100_000)));
        path.extend((1..=4).map(|i| Point::new(1_800_000 + i * 100_000, 0)));
        let out = straighten(&path, LayerId(0), &check);
        assert!(out.len() < path.len());
        for w in out.windows(2) {
            assert!(
                check.segment_is_clear(LayerId(0), Segment::new(w[0], w[1])),
                "{w:?}"
            );
            let (dx, dy) = (w[1].x - w[0].x, w[1].y - w[0].y);
            assert!(
                dx == 0 || dy == 0 || dx.abs() == dy.abs(),
                "not octilinear: {w:?}"
            );
        }
    }
}
