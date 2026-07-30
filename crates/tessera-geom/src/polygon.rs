use crate::point::Point;
use crate::segment::Segment;

/// A simple (non-self-intersecting) closed polygon, in nanometres — the
/// primitive rule-area / keepout zone outlines need (plan §7.5.6,
/// `docs/DECISIONS.md` ADR-0002 Q4). Vertices are listed in order; the
/// polygon is implicitly closed (the last vertex connects back to the
/// first) — callers must not repeat the first vertex at the end.
///
/// Self-intersection isn't checked (the exact predicates below don't
/// assume simplicity, only that "inside" means "odd crossing count," which
/// is well-defined regardless) — a genuinely self-intersecting outline
/// would just make "inside" mean something less intuitive, not panic or
/// behave incorrectly.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Polygon {
    pub vertices: Vec<Point>,
}

impl Polygon {
    #[must_use]
    pub fn new(vertices: Vec<Point>) -> Self {
        debug_assert!(vertices.len() >= 3, "a polygon needs at least 3 vertices");
        Self { vertices }
    }

    fn edges(&self) -> impl Iterator<Item = Segment> + '_ {
        let n = self.vertices.len();
        (0..n).map(move |i| Segment::new(self.vertices[i], self.vertices[(i + 1) % n]))
    }

    /// True iff `point` is inside this polygon, by the standard exact
    /// crossing-number test: a horizontal ray from `point` in the +x
    /// direction, counting how many edges it crosses — odd means inside.
    /// Kept fully exact via `i128` cross-multiplication (no division, no
    /// float), matching every other predicate in this crate.
    ///
    /// Like the classic algorithm this is based on, a point exactly on an
    /// edge may resolve to either side — this crate has no third
    /// "exactly on the boundary" answer, only inside/outside. Callers
    /// that need to know about touching a specific segment (rather than
    /// "is this one point inside") should use
    /// [`Polygon::intersects_segment`] instead, which does account for
    /// boundary contact.
    #[must_use]
    pub fn contains_point(&self, point: Point) -> bool {
        let mut inside = false;
        let n = self.vertices.len();
        for i in 0..n {
            let a = self.vertices[i];
            let b = self.vertices[(i + 1) % n];
            if (a.y > point.y) != (b.y > point.y) {
                // Edge (a, b) straddles the horizontal line y = point.y.
                // Compare point.x against the edge's x at that y without
                // dividing: cross-multiply by dy, flipping the
                // comparison when dy is negative so the inequality
                // direction stays correct.
                let dy = i128::from(b.y - a.y);
                let dx = i128::from(b.x - a.x);
                let lhs = i128::from(point.x - a.x) * dy;
                let rhs = dx * i128::from(point.y - a.y);
                let crosses_to_the_right = if dy > 0 { lhs < rhs } else { lhs > rhs };
                if crosses_to_the_right {
                    inside = !inside;
                }
            }
        }
        inside
    }

    /// True iff `segment` shares at least one point with this polygon —
    /// its interior (per [`Polygon::contains_point`]) or its boundary (any
    /// edge `segment` touches or crosses, per [`Segment::intersects`]).
    /// This is deliberately a single "touches at all" test rather than
    /// separate "fully inside" vs. "intersects" tests: empirically, real
    /// KiCad's `insideArea`/`intersectsArea` custom-rule predicates behave
    /// identically for track items (`docs/DECISIONS.md`'s "ADR-0002
    /// addendum" entry) — both match on any overlap, not full containment
    /// — so there is currently nothing to gain from a separate
    /// fully-contained test.
    #[must_use]
    pub fn intersects_segment(&self, segment: Segment) -> bool {
        if self.contains_point(segment.a) || self.contains_point(segment.b) {
            return true;
        }
        self.edges().any(|edge| edge.intersects(segment))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_square() -> Polygon {
        Polygon::new(vec![
            Point::new(0, 0),
            Point::new(10, 0),
            Point::new(10, 10),
            Point::new(0, 10),
        ])
    }

    #[test]
    fn contains_a_point_solidly_inside() {
        assert!(unit_square().contains_point(Point::new(5, 5)));
    }

    #[test]
    fn does_not_contain_a_point_solidly_outside() {
        assert!(!unit_square().contains_point(Point::new(20, 20)));
    }

    #[test]
    fn does_not_contain_a_point_outside_but_aligned_with_an_edge() {
        // Same y as the square's interior, but far to the right — a classic
        // case a buggy ray-cast can get wrong if it mishandles the
        // horizontal extent of the ray.
        assert!(!unit_square().contains_point(Point::new(50, 5)));
    }

    #[test]
    fn concave_polygon_excludes_its_notch() {
        // A "C" shape: a square with a notch bitten out of its right side.
        let c_shape = Polygon::new(vec![
            Point::new(0, 0),
            Point::new(10, 0),
            Point::new(10, 4),
            Point::new(5, 4),
            Point::new(5, 6),
            Point::new(10, 6),
            Point::new(10, 10),
            Point::new(0, 10),
        ]);
        assert!(c_shape.contains_point(Point::new(2, 5)), "inside the body");
        assert!(
            !c_shape.contains_point(Point::new(8, 5)),
            "inside the notch, not the body"
        );
    }

    #[test]
    fn intersects_segment_fully_inside() {
        let seg = Segment::new(Point::new(2, 2), Point::new(8, 2));
        assert!(unit_square().intersects_segment(seg));
    }

    #[test]
    fn intersects_segment_straddling_the_boundary() {
        let seg = Segment::new(Point::new(5, 5), Point::new(15, 5));
        assert!(unit_square().intersects_segment(seg));
    }

    #[test]
    fn does_not_intersect_a_segment_fully_outside() {
        let seg = Segment::new(Point::new(20, 20), Point::new(30, 20));
        assert!(!unit_square().intersects_segment(seg));
    }

    #[test]
    fn intersects_a_segment_that_only_touches_an_edge() {
        // Passes just outside the square, but crosses the boundary line
        // exactly at one edge's endpoint.
        let seg = Segment::new(Point::new(10, 0), Point::new(20, 5));
        assert!(unit_square().intersects_segment(seg));
    }

    #[test]
    fn does_not_intersect_a_segment_that_passes_nearby_without_touching() {
        let seg = Segment::new(Point::new(11, -5), Point::new(11, 15));
        assert!(!unit_square().intersects_segment(seg));
    }
}
