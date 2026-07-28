use crate::point::Point;
use crate::predicates::{orient, Orientation};

/// A straight track segment between two endpoints, in nanometres.
///
/// KiCad also has arc tracks (plan §4.1: "model them natively; do not
/// silently polygonise, or you will fail clearance checks by fractions of a
/// micron"). Arc support is a deliberate omission here, tracked for a
/// follow-up once straight-segment clearance is solid — polygonising early
/// would bake in exactly the failure mode the plan warns against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Segment {
    pub a: Point,
    pub b: Point,
}

/// An exact squared distance, kept as a fraction rather than divided down.
///
/// `denominator` is always strictly positive. Never convert this to a float
/// or perform the division to "simplify" it — every consumer in this crate
/// compares two such values (or a value against `threshold^2`) by
/// cross-multiplication, which is the only way the comparison stays exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RationalDistanceSq {
    pub numerator: i128,
    pub denominator: i128,
}

impl RationalDistanceSq {
    /// True iff this squared distance is at least `threshold_sq`, decided by
    /// cross-multiplication so no division (and no precision loss) ever
    /// happens. This is the shape every DRC clearance check should take.
    #[must_use]
    pub const fn at_least(self, threshold_sq: i128) -> bool {
        // denominator > 0 is a struct invariant, so the inequality direction
        // is preserved by this cross-multiplication.
        self.numerator >= threshold_sq * self.denominator
    }
}

impl Segment {
    #[must_use]
    pub const fn new(a: Point, b: Point) -> Self {
        Self { a, b }
    }

    /// Exact squared distance from `p` to the closest point on this segment.
    ///
    /// Classic clamped-projection algorithm, kept as an exact fraction: let
    /// `t = ((p - a) . (b - a)) / |b - a|^2` be the projection parameter,
    /// clamped to `[0, 1]`. Rather than compute `t` (which is generally
    /// irrational-looking as a ratio and would force a float), the clamp
    /// decision and the final distance are both expressed as comparisons
    /// and a fraction in the *unclamped* numerator/denominator, so nothing
    /// here ever divides.
    #[must_use]
    pub fn distance_sq(self, p: Point) -> RationalDistanceSq {
        let ab = self.b.sub(self.a);
        let ap = p.sub(self.a);
        let denominator = ab.length_sq();

        if denominator == 0 {
            // Degenerate segment (a == b): distance to a point.
            return RationalDistanceSq {
                numerator: ap.length_sq(),
                denominator: 1,
            };
        }

        let t_num = ap.dot(ab); // t = t_num / denominator, denominator > 0

        if t_num <= 0 {
            // Closest point is `a`.
            RationalDistanceSq {
                numerator: ap.length_sq(),
                denominator: 1,
            }
        } else if t_num >= denominator {
            // Closest point is `b`.
            RationalDistanceSq {
                numerator: p.sub(self.b).length_sq(),
                denominator: 1,
            }
        } else {
            // Perpendicular distance: |ap|^2 - t_num^2 / denominator, kept
            // as a single fraction over `denominator`.
            RationalDistanceSq {
                numerator: ap.length_sq() * denominator - t_num * t_num,
                denominator,
            }
        }
    }

    /// True iff every point on this segment is at least `min_nm` away from
    /// `p`. `min_nm` must be non-negative; this is the primitive
    /// `tessera-drc` clearance checks are built on.
    #[must_use]
    pub fn clears_point(self, p: Point, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0, "clearance distance must be non-negative");
        let threshold_sq = i128::from(min_nm) * i128::from(min_nm);
        self.distance_sq(p).at_least(threshold_sq)
    }

    /// True iff this segment and `other` share at least one point (touch or
    /// cross), decided exactly via [`orient`] — no epsilon, so a crossing at
    /// any angle including a shared endpoint or an overlapping collinear
    /// stretch is detected precisely.
    ///
    /// Standard four-orientation test plus the three collinear special
    /// cases (segments that touch but don't "straddle" each other in the
    /// general sense).
    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        let (p1, q1) = (self.a, self.b);
        let (p2, q2) = (other.a, other.b);

        let o1 = orient(p1, q1, p2);
        let o2 = orient(p1, q1, q2);
        let o3 = orient(p2, q2, p1);
        let o4 = orient(p2, q2, q1);

        if o1 != o2 && o3 != o4 {
            return true;
        }

        (o1 == Orientation::Collinear && Self::on_segment(p1, p2, q1))
            || (o2 == Orientation::Collinear && Self::on_segment(p1, q2, q1))
            || (o3 == Orientation::Collinear && Self::on_segment(p2, p1, q2))
            || (o4 == Orientation::Collinear && Self::on_segment(p2, q1, q2))
    }

    /// Given that `p`, `q`, `r` are already known to be collinear, is `q`
    /// within `p`..=`r`'s bounding box (equivalently, on the segment `p-r`)?
    fn on_segment(p: Point, q: Point, r: Point) -> bool {
        q.x <= p.x.max(r.x) && q.x >= p.x.min(r.x) && q.y <= p.y.max(r.y) && q.y >= p.y.min(r.y)
    }

    /// True iff every point on this segment is at least `min_nm` away from
    /// every point on `other`.
    ///
    /// Deliberately does **not** compute "the" minimum distance between the
    /// two segments and compare it once: once intersection is ruled out,
    /// the minimum is always achieved at one of the four endpoint-to-
    /// opposite-segment distances, but *finding which one* would mean
    /// comparing two [`RationalDistanceSq`] fractions against each other —
    /// and unlike comparing a fraction against a small `threshold_sq` (safe;
    /// see [`RationalDistanceSq::at_least`]), comparing two such fractions
    /// multiplies two coordinate-scale numerators together and overflows
    /// `i128` well within [`crate::MAX_COORDINATE_NM`]'s supported range.
    /// "Minimum of four is >= threshold" is logically identical to "every
    /// one of the four is >= threshold," and the latter only ever compares
    /// each fraction against the (small) threshold, which is safe.
    #[must_use]
    pub fn clears_segment(self, other: Self, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0, "clearance distance must be non-negative");
        if self.intersects(other) {
            return min_nm <= 0;
        }
        let threshold_sq = i128::from(min_nm) * i128::from(min_nm);
        other.distance_sq(self.a).at_least(threshold_sq)
            && other.distance_sq(self.b).at_least(threshold_sq)
            && self.distance_sq(other.a).at_least(threshold_sq)
            && self.distance_sq(other.b).at_least(threshold_sq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_to_perpendicular_foot() {
        let seg = Segment::new(Point::new(0, 0), Point::new(10, 0));
        let d = seg.distance_sq(Point::new(5, 3));
        assert_eq!(d.numerator, 9 * d.denominator); // exactly 3^2 away
    }

    #[test]
    fn distance_clamped_to_endpoint() {
        let seg = Segment::new(Point::new(0, 0), Point::new(10, 0));
        // Beyond `b`, closest point is `b` itself: distance is exactly 5.
        let d = seg.distance_sq(Point::new(13, 4));
        assert_eq!(d.numerator, 25 * d.denominator);
    }

    #[test]
    fn degenerate_segment_is_a_point() {
        let seg = Segment::new(Point::new(2, 2), Point::new(2, 2));
        let d = seg.distance_sq(Point::new(5, 6));
        assert_eq!(d.numerator, 25); // 3^2 + 4^2
        assert_eq!(d.denominator, 1);
    }

    #[test]
    fn clears_point_exact_boundary() {
        let seg = Segment::new(Point::new(0, 0), Point::new(10, 0));
        // Point is exactly 3 away from the segment (perpendicular foot).
        let p = Point::new(5, 3);
        assert!(seg.clears_point(p, 3));
        assert!(!seg.clears_point(p, 4));
    }

    #[test]
    fn point_on_segment_has_zero_clearance() {
        let seg = Segment::new(Point::new(0, 0), Point::new(10, 0));
        assert!(seg.clears_point(Point::new(5, 0), 0));
        assert!(!seg.clears_point(Point::new(5, 0), 1));
    }

    #[test]
    fn crossing_segments_intersect() {
        let a = Segment::new(Point::new(0, 0), Point::new(10, 10));
        let b = Segment::new(Point::new(0, 10), Point::new(10, 0));
        assert!(a.intersects(b));
        assert!(b.intersects(a));
    }

    #[test]
    fn parallel_segments_do_not_intersect() {
        let a = Segment::new(Point::new(0, 0), Point::new(10, 0));
        let b = Segment::new(Point::new(0, 5), Point::new(10, 5));
        assert!(!a.intersects(b));
    }

    #[test]
    fn touching_endpoint_intersects() {
        let a = Segment::new(Point::new(0, 0), Point::new(10, 0));
        let b = Segment::new(Point::new(10, 0), Point::new(10, 10));
        assert!(a.intersects(b));
    }

    #[test]
    fn collinear_overlap_intersects() {
        let a = Segment::new(Point::new(0, 0), Point::new(10, 0));
        let b = Segment::new(Point::new(5, 0), Point::new(15, 0));
        assert!(a.intersects(b));
    }

    #[test]
    fn collinear_non_overlapping_does_not_intersect() {
        let a = Segment::new(Point::new(0, 0), Point::new(10, 0));
        let b = Segment::new(Point::new(20, 0), Point::new(30, 0));
        assert!(!a.intersects(b));
    }

    #[test]
    fn crossing_segments_have_zero_clearance() {
        let a = Segment::new(Point::new(0, 0), Point::new(10, 10));
        let b = Segment::new(Point::new(0, 10), Point::new(10, 0));
        assert!(a.clears_segment(b, 0));
        assert!(!a.clears_segment(b, 1));
    }

    #[test]
    fn parallel_segment_clearance_is_the_gap() {
        let a = Segment::new(Point::new(0, 0), Point::new(10, 0));
        let b = Segment::new(Point::new(0, 5), Point::new(10, 5));
        assert!(a.clears_segment(b, 5));
        assert!(!a.clears_segment(b, 6));
    }
}
