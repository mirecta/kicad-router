use crate::circle::Circle;
use crate::point::Point;
use crate::rect::Rect;
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

    /// Axis-aligned bounds `(min, max)`, grown by `margin_nm` on every side.
    #[must_use]
    pub fn bounding_box(&self, margin_nm: i64) -> (Point, Point) {
        let xs = self.vertices.iter().map(|p| p.x);
        let ys = self.vertices.iter().map(|p| p.y);
        let (min_x, max_x) = xs.fold((i64::MAX, i64::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
        let (min_y, max_y) = ys.fold((i64::MAX, i64::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));
        (
            Point::new(min_x - margin_nm, min_y - margin_nm),
            Point::new(max_x + margin_nm, max_y + margin_nm),
        )
    }

    // The clearance predicates below treat the polygon as a filled region:
    // anything inside it doesn't clear, and outside it the distance is to
    // the nearest edge. KiCad stores a zone fill with holes as one
    // "fractured" outline whose slits run through copper, so the slit
    // edges are never nearer to an outside item than real copper is.

    /// True iff `point` lies outside this polygon, at least `min_nm` from it.
    #[must_use]
    pub fn clears_point(&self, point: Point, min_nm: i64) -> bool {
        !self.contains_point(point) && self.edges().all(|e| e.clears_point(point, min_nm))
    }

    #[must_use]
    pub fn clears_circle(&self, circle: Circle, min_nm: i64) -> bool {
        self.clears_point(circle.center, min_nm.saturating_add(circle.radius_nm))
    }

    /// True iff all of `segment` lies outside this polygon, at least
    /// `min_nm` from it.
    #[must_use]
    pub fn clears_segment(&self, segment: Segment, min_nm: i64) -> bool {
        !self.intersects_segment(segment) && self.edges().all(|e| e.clears_segment(segment, min_nm))
    }

    /// True iff the two polygons don't overlap and every point of one is at
    /// least `min_nm` from the other: neither contains a vertex of the other
    /// (so neither lies inside the other) and every pair of edges clears.
    #[must_use]
    pub fn clears_polygon(&self, other: &Self, min_nm: i64) -> bool {
        let inside = |a: &Self, b: &Self| b.vertices.first().is_some_and(|&v| a.contains_point(v));
        !inside(self, other)
            && !inside(other, self)
            && self
                .edges()
                .all(|e| other.edges().all(|f| e.clears_segment(f, min_nm)))
    }

    /// True iff `rect` lies wholly outside this polygon, at least `min_nm`
    /// from it — a rect containing the whole polygon fails on its edges, a
    /// polygon containing the whole rect fails on the rect's centre.
    #[must_use]
    pub fn clears_rect(&self, rect: Rect, min_nm: i64) -> bool {
        !self.contains_point(rect.center) && self.edges().all(|e| rect.clears_segment(e, min_nm))
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

    fn big_square() -> Polygon {
        Polygon::new(vec![
            Point::new(0, 0),
            Point::new(1_000_000, 0),
            Point::new(1_000_000, 1_000_000),
            Point::new(0, 1_000_000),
        ])
    }

    #[test]
    fn clears_point_measures_to_the_nearest_edge_and_rejects_inside() {
        let sq = big_square();
        assert!(sq.clears_point(Point::new(1_200_000, 500_000), 200_000));
        assert!(!sq.clears_point(Point::new(1_200_000, 500_000), 200_001));
        // 3-4-5 off the corner: exactly 500_000 away.
        assert!(sq.clears_point(Point::new(1_300_000, 1_400_000), 500_000));
        assert!(!sq.clears_point(Point::new(1_300_000, 1_400_000), 500_001));
        assert!(!sq.clears_point(Point::new(500_000, 500_000), 0));
    }

    #[test]
    fn clears_segment_rejects_crossing_and_fully_inside_segments() {
        let sq = big_square();
        let outside = Segment::new(Point::new(1_100_000, 0), Point::new(1_100_000, 1_000_000));
        assert!(sq.clears_segment(outside, 100_000));
        assert!(!sq.clears_segment(outside, 100_001));
        let crossing = Segment::new(Point::new(500_000, 500_000), Point::new(2_000_000, 500_000));
        assert!(!sq.clears_segment(crossing, 0));
        let inside = Segment::new(Point::new(400_000, 400_000), Point::new(600_000, 600_000));
        assert!(!sq.clears_segment(inside, 0));
    }

    #[test]
    fn clears_polygon_measures_edge_to_edge_and_rejects_containment() {
        let sq = big_square();
        let right = Polygon::new(vec![
            Point::new(1_300_000, 0),
            Point::new(2_000_000, 0),
            Point::new(2_000_000, 1_000_000),
            Point::new(1_300_000, 1_000_000),
        ]);
        assert!(sq.clears_polygon(&right, 300_000));
        assert!(!sq.clears_polygon(&right, 300_001));
        let inner = Polygon::new(vec![
            Point::new(400_000, 400_000),
            Point::new(600_000, 400_000),
            Point::new(600_000, 600_000),
        ]);
        assert!(!sq.clears_polygon(&inner, 0));
        assert!(!inner.clears_polygon(&sq, 0));
    }

    #[test]
    fn clears_rect_and_circle() {
        let sq = big_square();
        let rect = Rect::new(Point::new(1_500_000, 500_000), 400_000, 400_000); // left edge 1_300_000
        assert!(sq.clears_rect(rect, 300_000));
        assert!(!sq.clears_rect(rect, 300_001));
        let around = Rect::new(Point::new(500_000, 500_000), 3_000_000, 3_000_000);
        assert!(!sq.clears_rect(around, 0));
        let circle = Circle::new(Point::new(1_500_000, 500_000), 200_000);
        assert!(sq.clears_circle(circle, 300_000));
        assert!(!sq.clears_circle(circle, 300_001));
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
