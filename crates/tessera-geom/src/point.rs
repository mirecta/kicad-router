/// The largest coordinate magnitude (in nanometres) this crate's exact
/// predicates guarantee to handle without integer overflow, on either axis.
///
/// KiCad's own on-board coordinate type (`VECTOR2I`, verified against
/// `libs/kimath/include/math/vector2d.h` in the KiCad source: `typedef
/// VECTOR2<int32_t> VECTOR2I`) tops out at `i32::MAX` nm (~2.147 m). This
/// crate deliberately supports a *smaller* range than that hard ceiling:
/// [`Segment::distance_sq`]'s clamped-projection branch multiplies two
/// already-squared lengths together (`|ap|^2 * |ab|^2`), and at KiCad's full
/// legal range that product overflows `i128` (worst case ~1.36e39 against an
/// `i128::MAX` of ~1.70e38). Rather than reimplement 256-bit exact
/// arithmetic — real, but wasted, complexity for a board size no physical
/// PCB will ever approach — the crate documents and enforces (via
/// `debug_assert!` in [`Point::new`]) a 1 m per-axis bound instead. At that
/// bound the same worst-case product is ~6.4e37, comfortably inside `i128`.
///
/// If a future corpus board legitimately needs more range, the fix is exact
/// wide (256-bit) arithmetic in [`Segment::distance_sq`](crate::Segment::distance_sq),
/// not raising this constant — raise it only alongside that work.
pub const MAX_COORDINATE_NM: i64 = 1_000_000_000;

/// A point in board space, in integer nanometres — KiCad's native unit.
///
/// Coordinates are `i64` throughout the geometry kernel's public API; `f64`
/// is reserved for cost heuristics elsewhere in the workspace and must never
/// appear in a predicate or in stored geometry (plan §4.1). Coordinates must
/// fit within [`MAX_COORDINATE_NM`] of the origin on each axis; see that
/// constant's docs for why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Point {
    pub x: i64,
    pub y: i64,
}

impl Point {
    #[must_use]
    pub const fn new(x: i64, y: i64) -> Self {
        debug_assert!(
            x >= -MAX_COORDINATE_NM && x <= MAX_COORDINATE_NM,
            "Point::x exceeds MAX_COORDINATE_NM"
        );
        debug_assert!(
            y >= -MAX_COORDINATE_NM && y <= MAX_COORDINATE_NM,
            "Point::y exceeds MAX_COORDINATE_NM"
        );
        Self { x, y }
    }

    #[must_use]
    pub const fn origin() -> Self {
        Self::new(0, 0)
    }

    #[must_use]
    pub const fn sub(self, other: Self) -> Vector {
        Vector::new(self.x - other.x, self.y - other.y)
    }

    #[must_use]
    pub const fn add(self, v: Vector) -> Self {
        Self::new(self.x + v.x, self.y + v.y)
    }
}

/// A displacement in board space, in integer nanometres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Vector {
    pub x: i64,
    pub y: i64,
}

impl Vector {
    #[must_use]
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }

    /// Exact squared length as `i128`; `i64` overflows for boards near their
    /// physical size limit once squared, so every product in this crate's
    /// predicates is carried in `i128` (plan §4.1's exactness requirement).
    #[must_use]
    pub const fn length_sq(self) -> i128 {
        let x = self.x as i128;
        let y = self.y as i128;
        x * x + y * y
    }

    /// Exact 2D cross product (the z-component of the 3D cross product),
    /// as `i128`. Its sign is the orientation predicate: positive means
    /// `other` is counter-clockwise from `self`.
    #[must_use]
    pub const fn cross(self, other: Self) -> i128 {
        self.x as i128 * other.y as i128 - self.y as i128 * other.x as i128
    }

    /// Exact dot product, as `i128`.
    #[must_use]
    pub const fn dot(self, other: Self) -> i128 {
        self.x as i128 * other.x as i128 + self.y as i128 * other.y as i128
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_add_roundtrip() {
        let a = Point::new(10, 20);
        let b = Point::new(3, -7);
        assert_eq!(b.add(a.sub(b)), a);
    }

    #[test]
    fn length_sq_matches_pythagoras() {
        assert_eq!(Vector::new(3, 4).length_sq(), 25);
        assert_eq!(Vector::new(-3, 4).length_sq(), 25);
    }

    #[test]
    fn cross_is_antisymmetric_and_zero_for_parallel() {
        let a = Vector::new(2, 0);
        let b = Vector::new(0, 3);
        assert_eq!(a.cross(b), 6);
        assert_eq!(b.cross(a), -6);
        assert_eq!(a.cross(Vector::new(4, 0)), 0);
    }

    #[test]
    fn no_overflow_at_max_coordinate() {
        // Exercise the full supported range (see MAX_COORDINATE_NM's docs)
        // to confirm the i128 intermediates hold at the boundary this crate
        // actually promises to support, not an arbitrary larger value.
        let big = MAX_COORDINATE_NM;
        let v = Vector::new(big, big);
        let _ = v.length_sq();
        let _ = v.cross(Vector::new(-big, big));
        let _ = v.dot(Vector::new(-big, big));
    }
}
