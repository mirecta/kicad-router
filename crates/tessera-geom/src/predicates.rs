use std::cmp::Ordering;

use crate::point::Point;

/// The three possible orderings of three points, by the sign of the exact
/// 2D cross product `(b - a) x (c - a)`.
///
/// This is *the* orientation predicate referenced throughout plan §4.1 —
/// every other exact predicate in this crate (segment intersection,
/// point-in-polygon, later CDT work) is built on it, so its correctness on
/// degenerate/collinear input matters more than almost anything else in the
/// geometry kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    CounterClockwise,
    Clockwise,
    Collinear,
}

/// Exact orientation of `c` relative to the directed line `a -> b`.
///
/// Uses `i128` intermediates (via [`Vector::cross`](crate::point::Vector::cross))
/// so the result is exact for every coordinate magnitude a real board can
/// produce — there is no epsilon here and there must never be one; the whole
/// point of an integer-nanometre kernel is that "collinear" is a fact, not a
/// judgment call (plan §4.1).
#[must_use]
pub fn orient(a: Point, b: Point, c: Point) -> Orientation {
    let cross = b.sub(a).cross(c.sub(a));
    match cross.cmp(&0) {
        Ordering::Greater => Orientation::CounterClockwise,
        Ordering::Less => Orientation::Clockwise,
        Ordering::Equal => Orientation::Collinear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ccw_triangle() {
        let a = Point::new(0, 0);
        let b = Point::new(1, 0);
        let c = Point::new(0, 1);
        assert_eq!(orient(a, b, c), Orientation::CounterClockwise);
        assert_eq!(orient(a, c, b), Orientation::Clockwise);
    }

    #[test]
    fn collinear_is_exact_not_approximate() {
        // A classic near-miss for float-based predicates: these three points
        // are exactly collinear, but naive f64 slope/cross computations can
        // report a nonzero epsilon here. Integer arithmetic must not.
        let a = Point::new(0, 0);
        let b = Point::new(1_000_003, 3_000_009);
        let c = Point::new(2_000_006, 6_000_018);
        assert_eq!(orient(a, b, c), Orientation::Collinear);
    }

    #[test]
    fn degenerate_a_equals_b() {
        let a = Point::new(5, 5);
        assert_eq!(orient(a, a, Point::new(0, 0)), Orientation::Collinear);
    }
}
