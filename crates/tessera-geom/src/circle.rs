use crate::point::Point;
use crate::segment::Segment;

/// A circular shape — a via, a round pad, or a round-ended track's swept
/// clearance envelope — centred at `center` with radius `radius_nm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Circle {
    pub center: Point,
    pub radius_nm: i64,
}

impl Circle {
    #[must_use]
    pub const fn new(center: Point, radius_nm: i64) -> Self {
        debug_assert!(radius_nm >= 0);
        Self { center, radius_nm }
    }

    /// True iff the gap between this circle's edge and `other`'s edge is at
    /// least `min_nm` — i.e. `distance(centers) - r1 - r2 >= min_nm`.
    ///
    /// Rearranged to `distance(centers) >= min_nm + r1 + r2` so the check
    /// stays a single squared-distance comparison (exact, no subtraction of
    /// a computed square root).
    #[must_use]
    pub fn clears_circle(self, other: Self, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        let threshold = min_nm
            .saturating_add(self.radius_nm)
            .saturating_add(other.radius_nm);
        let threshold_sq = i128::from(threshold) * i128::from(threshold);
        let dist_sq = self.center.sub(other.center).length_sq();
        dist_sq >= threshold_sq
    }

    /// True iff every point on `segment` clears this circle's edge by at
    /// least `min_nm`.
    #[must_use]
    pub fn clears_segment(self, segment: Segment, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        let threshold = min_nm.saturating_add(self.radius_nm);
        segment.clears_point(self.center, threshold)
    }

    /// True iff `point` is at least `min_nm` from this circle's edge.
    #[must_use]
    pub fn clears_point(self, point: Point, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        let threshold = min_nm.saturating_add(self.radius_nm);
        let threshold_sq = i128::from(threshold) * i128::from(threshold);
        self.center.sub(point).length_sq() >= threshold_sq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_vias_touching_is_zero_clearance() {
        // Centers 2000nm apart, both radius 1000nm: edges exactly touch.
        let a = Circle::new(Point::new(0, 0), 1000);
        let b = Circle::new(Point::new(2000, 0), 1000);
        assert!(a.clears_circle(b, 0));
        assert!(!a.clears_circle(b, 1));
    }

    #[test]
    fn clears_circle_is_symmetric() {
        let a = Circle::new(Point::new(0, 0), 500);
        let b = Circle::new(Point::new(3000, 4000), 700);
        assert_eq!(a.clears_circle(b, 100), b.clears_circle(a, 100));
    }

    #[test]
    fn segment_clearance_accounts_for_radius() {
        let seg = Segment::new(Point::new(0, 0), Point::new(100, 0));
        let via = Circle::new(Point::new(50, 10), 5);
        // Perpendicular distance from via center to segment is 10; minus the
        // via's own radius (5), the true clearance is 5.
        assert!(via.clears_segment(seg, 5));
        assert!(!via.clears_segment(seg, 6));
    }
}
