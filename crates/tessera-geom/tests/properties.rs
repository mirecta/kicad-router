use proptest::prelude::*;
use tessera_geom::{orient, Circle, Orientation, Point, Segment, MAX_COORDINATE_NM};

// Exercise the crate's full documented coordinate range (see
// MAX_COORDINATE_NM's docs for why it's bounded below KiCad's theoretical
// int32-nm limit) rather than an arbitrary smaller or larger range.
const COORD: std::ops::RangeInclusive<i64> = -MAX_COORDINATE_NM..=MAX_COORDINATE_NM;
const RADIUS: std::ops::RangeInclusive<i64> = 0..=5_000_000;
const CLEARANCE: std::ops::RangeInclusive<i64> = 0..=1_000_000;

fn point_strategy() -> impl Strategy<Value = Point> {
    (COORD, COORD).prop_map(|(x, y)| Point::new(x, y))
}

fn segment_strategy() -> impl Strategy<Value = Segment> {
    (point_strategy(), point_strategy()).prop_map(|(a, b)| Segment::new(a, b))
}

fn circle_strategy() -> impl Strategy<Value = Circle> {
    (point_strategy(), RADIUS).prop_map(|(c, r)| Circle::new(c, r))
}

proptest! {
    #[test]
    fn orient_is_antisymmetric(a in point_strategy(), b in point_strategy(), c in point_strategy()) {
        let forward = orient(a, b, c);
        let swapped = orient(a, c, b);
        match forward {
            Orientation::CounterClockwise => prop_assert_eq!(swapped, Orientation::Clockwise),
            Orientation::Clockwise => prop_assert_eq!(swapped, Orientation::CounterClockwise),
            Orientation::Collinear => prop_assert_eq!(swapped, Orientation::Collinear),
        }
    }

    #[test]
    fn orient_a_a_b_is_always_collinear(a in point_strategy(), b in point_strategy()) {
        prop_assert_eq!(orient(a, a, b), Orientation::Collinear);
    }

    #[test]
    fn segment_distance_sq_is_never_negative(seg in segment_strategy(), p in point_strategy()) {
        let d = seg.distance_sq(p);
        prop_assert!(d.numerator >= 0);
        prop_assert!(d.denominator > 0);
    }

    #[test]
    fn segment_endpoint_clearance_is_zero(seg in segment_strategy()) {
        // A segment always has zero clearance from its own endpoints.
        prop_assert!(seg.clears_point(seg.a, 0));
        prop_assert!(seg.clears_point(seg.b, 0));
    }

    #[test]
    fn circle_clears_circle_is_symmetric(a in circle_strategy(), b in circle_strategy(), min in CLEARANCE) {
        prop_assert_eq!(a.clears_circle(b, min), b.clears_circle(a, min));
    }

    #[test]
    fn circle_clears_itself_only_if_clearance_is_nonpositive_after_radii(
        c in circle_strategy(), min in CLEARANCE
    ) {
        // A circle against an identical copy of itself: distance between
        // centers is zero, so it "clears" only when min_nm <= 0 (radii
        // exactly cancel iff min_nm == 0, since distance - 2r >= min_nm
        // becomes -2r >= min_nm, impossible for r > 0 and min_nm >= 0
        // unless min_nm == 0 and r == 0).
        let result = c.clears_circle(c, min);
        let expected = min == 0 && c.radius_nm == 0;
        prop_assert_eq!(result, expected);
    }

    #[test]
    fn tighter_clearance_is_monotonically_harder_to_satisfy(
        seg in segment_strategy(), p in point_strategy(), min in CLEARANCE
    ) {
        // If a larger minimum clearance is satisfied, every smaller minimum
        // must also be satisfied (monotonicity of the clearance predicate).
        if min > 0 && seg.clears_point(p, min) {
            prop_assert!(seg.clears_point(p, min - 1));
        }
    }

    #[test]
    fn segment_intersects_is_symmetric(a in segment_strategy(), b in segment_strategy()) {
        prop_assert_eq!(a.intersects(b), b.intersects(a));
    }

    #[test]
    fn segment_intersects_itself(seg in segment_strategy()) {
        prop_assert!(seg.intersects(seg));
    }

    #[test]
    fn intersecting_segments_never_clear_a_positive_minimum(
        a in segment_strategy(), b in segment_strategy()
    ) {
        if a.intersects(b) {
            prop_assert!(!a.clears_segment(b, 1));
        }
    }

    #[test]
    fn segment_clears_segment_is_symmetric(
        a in segment_strategy(), b in segment_strategy(), min in CLEARANCE
    ) {
        prop_assert_eq!(a.clears_segment(b, min), b.clears_segment(a, min));
    }

    #[test]
    fn segment_clears_itself_only_at_zero_clearance(seg in segment_strategy(), min in CLEARANCE) {
        prop_assert_eq!(seg.clears_segment(seg, min), min == 0);
    }

    #[test]
    fn tighter_segment_clearance_is_monotonically_harder_to_satisfy(
        a in segment_strategy(), b in segment_strategy(), min in CLEARANCE
    ) {
        if min > 0 && a.clears_segment(b, min) {
            prop_assert!(a.clears_segment(b, min - 1));
        }
    }
}
