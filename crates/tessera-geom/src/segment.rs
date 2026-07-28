use crate::point::Point;

/// A straight track segment between two endpoints, in nanometres.
///
/// KiCad also has arc tracks (plan §4.1: "model them natively; do not
/// silently polygonise, or you will fail clearance checks by fractions of a
/// micron"). Arc support is a deliberate omission here, tracked for a
/// follow-up once straight-segment clearance is solid — polygonising early
/// would bake in exactly the failure mode the plan warns against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}
