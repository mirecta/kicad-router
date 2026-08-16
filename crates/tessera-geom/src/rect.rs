use crate::circle::Circle;
use crate::point::Point;
use crate::segment::Segment;

/// An axis-aligned rectangular shape — a non-circular pad's copper
/// shape (KiCad's `rect`/`roundrect` pad shapes; roundrect's corner
/// rounding is not modelled, an accepted over-approximation, same
/// reasoning as circle-only clearance treating a via's own copper as
/// its full bounding circle).
///
/// **Always axis-aligned, never rotated**: this workspace doesn't
/// model footprint rotation anywhere yet (see
/// `tessera-io-kicad::parser`'s module docs — a pad's absolute
/// position already assumes zero footprint rotation), so a rotated
/// real pad would be mis-measured here rather than silently corrected
/// — the same honest limitation as the existing "no footprint
/// rotation" scope note, not a new one specific to this shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub center: Point,
    pub half_width_nm: i64,
    pub half_height_nm: i64,
}

impl Rect {
    #[must_use]
    pub const fn new(center: Point, width_nm: i64, height_nm: i64) -> Self {
        debug_assert!(width_nm >= 0 && height_nm >= 0);
        Self {
            center,
            half_width_nm: width_nm / 2,
            half_height_nm: height_nm / 2,
        }
    }

    fn min_corner(self) -> Point {
        Point::new(
            self.center.x - self.half_width_nm,
            self.center.y - self.half_height_nm,
        )
    }

    fn max_corner(self) -> Point {
        Point::new(
            self.center.x + self.half_width_nm,
            self.center.y + self.half_height_nm,
        )
    }

    /// This rectangle's four corners, in order (suitable for use as a
    /// closed loop — each connects to the next, and the last back to
    /// the first).
    fn corners(self) -> [Point; 4] {
        let min = self.min_corner();
        let max = self.max_corner();
        [min, Point::new(max.x, min.y), max, Point::new(min.x, max.y)]
    }

    fn edges(self) -> [Segment; 4] {
        let c = self.corners();
        [
            Segment::new(c[0], c[1]),
            Segment::new(c[1], c[2]),
            Segment::new(c[2], c[3]),
            Segment::new(c[3], c[0]),
        ]
    }

    /// True iff `point` is inside or on the boundary of this rectangle.
    #[must_use]
    pub fn contains_point(self, point: Point) -> bool {
        let min = self.min_corner();
        let max = self.max_corner();
        point.x >= min.x && point.x <= max.x && point.y >= min.y && point.y <= max.y
    }

    /// Exact squared distance from `point` to the closest point on this
    /// rectangle (zero if `point` is inside or on the boundary) — the
    /// classic clamped-coordinate distance to an axis-aligned bounding
    /// box, kept in `i128` throughout (no division, no float; mirrors
    /// [`crate::Vector::length_sq`]'s exactness).
    #[must_use]
    pub fn distance_sq(self, point: Point) -> i128 {
        let min = self.min_corner();
        let max = self.max_corner();
        let dx = (min.x - point.x).max(0).max(point.x - max.x);
        let dy = (min.y - point.y).max(0).max(point.y - max.y);
        i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy)
    }

    /// True iff every point on this rectangle is at least `min_nm` from
    /// `point`.
    #[must_use]
    pub fn clears_point(self, point: Point, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        let threshold_sq = i128::from(min_nm) * i128::from(min_nm);
        self.distance_sq(point) >= threshold_sq
    }

    /// True iff the gap between this rectangle's edge and `circle`'s edge
    /// is at least `min_nm`.
    #[must_use]
    pub fn clears_circle(self, circle: Circle, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        let threshold = min_nm.saturating_add(circle.radius_nm);
        self.clears_point(circle.center, threshold)
    }

    /// True iff this rectangle and `other` overlap at all (touch or
    /// cross) — either rectangle's own bounding box overlaps the
    /// other's on both axes. Standard axis-aligned-rectangle overlap
    /// test.
    #[must_use]
    pub fn intersects_rect(self, other: Self) -> bool {
        let (a_min, a_max) = (self.min_corner(), self.max_corner());
        let (b_min, b_max) = (other.min_corner(), other.max_corner());
        a_min.x <= b_max.x && b_min.x <= a_max.x && a_min.y <= b_max.y && b_min.y <= a_max.y
    }

    /// True iff every point on this rectangle is at least `min_nm` from
    /// every point on `other`.
    #[must_use]
    pub fn clears_rect(self, other: Self, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        if self.intersects_rect(other) {
            return min_nm <= 0;
        }
        let (a_min, a_max) = (self.min_corner(), self.max_corner());
        let (b_min, b_max) = (other.min_corner(), other.max_corner());
        let dx = (a_min.x - b_max.x).max(0).max(b_min.x - a_max.x);
        let dy = (a_min.y - b_max.y).max(0).max(b_min.y - a_max.y);
        let dist_sq = i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy);
        let threshold_sq = i128::from(min_nm) * i128::from(min_nm);
        dist_sq >= threshold_sq
    }

    /// True iff this rectangle and `segment` share at least one point.
    #[must_use]
    pub fn intersects_segment(self, segment: Segment) -> bool {
        if self.contains_point(segment.a) || self.contains_point(segment.b) {
            return true;
        }
        self.edges().iter().any(|edge| edge.intersects(segment))
    }

    /// True iff every point on this rectangle is at least `min_nm` from
    /// every point on `segment`.
    ///
    /// Once intersection is ruled out, the minimum gap between a
    /// rectangle and a segment (both convex, straight-edged shapes) is
    /// always achieved at a rectangle-corner-to-segment or
    /// segment-endpoint-to-rectangle pairing — the same reasoning
    /// [`Segment::clears_segment`] documents for its own four-endpoint
    /// check, just with this rectangle's four corners standing in for
    /// a second segment's two endpoints.
    #[must_use]
    pub fn clears_segment(self, segment: Segment, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        if self.intersects_segment(segment) {
            return min_nm <= 0;
        }
        let threshold_sq = i128::from(min_nm) * i128::from(min_nm);
        self.clears_point(segment.a, min_nm)
            && self.clears_point(segment.b, min_nm)
            && self
                .corners()
                .into_iter()
                .all(|corner| segment.distance_sq(corner).at_least(threshold_sq))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_point_inside_and_outside() {
        let r = Rect::new(Point::new(0, 0), 2000, 1000);
        assert!(r.contains_point(Point::new(0, 0)));
        assert!(r.contains_point(Point::new(1000, 500))); // exactly on a corner
        assert!(!r.contains_point(Point::new(1001, 0)));
    }

    #[test]
    fn clears_point_matches_hand_computed_gap() {
        let r = Rect::new(Point::new(0, 0), 2000, 2000); // extends to +-1000 each axis
                                                         // Point straight out from the right edge, 500nm away.
        let p = Point::new(1500, 0);
        assert!(r.clears_point(p, 500));
        assert!(!r.clears_point(p, 501));
    }

    #[test]
    fn clears_point_from_a_diagonal_corner() {
        let r = Rect::new(Point::new(0, 0), 2000, 2000); // corners at (+-1000, +-1000)
                                                         // (1003, 1004): 3nm past the right edge, 4nm past the top edge ->
                                                         // exact 3-4-5 triangle distance of 5nm from the corner.
        let p = Point::new(1003, 1004);
        assert!(r.clears_point(p, 5));
        assert!(!r.clears_point(p, 6));
    }

    #[test]
    fn clears_circle_accounts_for_radius() {
        let r = Rect::new(Point::new(0, 0), 2000, 2000);
        let c = Circle::new(Point::new(1500, 0), 100);
        // Rect edge at x=1000, circle edge at x=1400: gap is 400.
        assert!(r.clears_circle(c, 400));
        assert!(!r.clears_circle(c, 401));
    }

    #[test]
    fn overlapping_rects_never_clear() {
        let a = Rect::new(Point::new(0, 0), 2000, 2000);
        let b = Rect::new(Point::new(500, 0), 2000, 2000);
        assert!(a.clears_rect(b, 0));
        assert!(!a.clears_rect(b, 1));
    }

    #[test]
    fn separated_rects_gap_is_the_axis_aligned_distance() {
        let a = Rect::new(Point::new(0, 0), 2000, 2000); // right edge at 1000
        let b = Rect::new(Point::new(3000, 0), 2000, 2000); // left edge at 2000
        assert!(a.clears_rect(b, 1000));
        assert!(!a.clears_rect(b, 1001));
    }

    #[test]
    fn diagonal_rect_gap_is_the_corner_to_corner_distance() {
        let a = Rect::new(Point::new(0, 0), 2000, 2000); // corner at (1000, 1000)
                                                         // b's nearest corner is at (1000+3, 1000+4) -> exact 3-4-5
                                                         // distance of 5 from a's corner.
        let b = Rect::new(Point::new(1503, 1504), 1000, 1000);
        assert!(a.clears_rect(b, 5));
        assert!(!a.clears_rect(b, 6));
    }

    #[test]
    fn crossing_segment_never_clears_a_rect() {
        let r = Rect::new(Point::new(0, 0), 2000, 2000);
        let seg = Segment::new(Point::new(-2000, 0), Point::new(2000, 0));
        assert!(r.clears_segment(seg, 0));
        assert!(!r.clears_segment(seg, 1));
    }

    #[test]
    fn parallel_segment_gap_matches_the_rect_edge_distance() {
        let r = Rect::new(Point::new(0, 0), 2000, 2000); // top edge at y=1000
        let seg = Segment::new(Point::new(-2000, 1500), Point::new(2000, 1500));
        assert!(r.clears_segment(seg, 500));
        assert!(!r.clears_segment(seg, 501));
    }

    #[test]
    fn segment_gap_measured_from_the_rects_own_corner() {
        let r = Rect::new(Point::new(0, 0), 2000, 2000); // corner at (1000, 1000)
                                                         // A short segment sitting diagonally past the corner, closest
                                                         // point exactly (1003, 1004) -> 3-4-5 distance of 5.
        let seg = Segment::new(Point::new(1003, 1004), Point::new(2000, 2000));
        assert!(r.clears_segment(seg, 5));
        assert!(!r.clears_segment(seg, 6));
    }
}
