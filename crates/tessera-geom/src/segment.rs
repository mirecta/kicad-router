use crate::point::{Point, Vector};
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

/// Two parallel track segments' geometric relationship — see
/// [`Segment::diff_pair_coupling`]'s docs for how this is measured and
/// what it's for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffPairCoupling {
    /// Centreline (not copper-edge) perpendicular distance between the
    /// two segments' lines.
    pub gap_nm: i64,
    /// The length, along the shared direction, where both segments run
    /// alongside each other.
    pub coupled_length_nm: i64,
    /// Both segments' own total length outside the coupled run, summed —
    /// zero if one segment's projection fully contains the other's.
    pub uncoupled_length_nm: i64,
}

impl Segment {
    #[must_use]
    pub const fn new(a: Point, b: Point) -> Self {
        Self { a, b }
    }

    /// This segment's own length, in nanometres, rounded to the nearest
    /// integer. **Reporting-only, never a predicate**: unlike every other
    /// method on this type, this one goes through `f64::sqrt`, per plan
    /// §4.1's rule that floats belong in cost/reporting values, never in
    /// geometric predicates or stored geometry. Coordinates are bounded by
    /// `MAX_COORDINATE_NM`, so the round-trip through `f64` here is
    /// lossless in practice (see [`crate::MAX_COORDINATE_NM`]'s docs for
    /// the same reasoning applied elsewhere in this crate). Exists for
    /// callers that need an actual length value to sum or display — e.g. a
    /// `.kicad_dru` `length` constraint's "actual" measurement, which is a
    /// reported quantity being compared against a bound, not a clearance
    /// predicate.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn length_nm(self) -> i64 {
        let length_sq = self.b.sub(self.a).length_sq();
        #[allow(clippy::cast_precision_loss)]
        let length_sq_f64 = length_sq as f64;
        length_sq_f64.sqrt().round() as i64
    }

    /// This segment's centreline coupling with `other` — the geometry a
    /// `.kicad_dru` `diff_pair_gap`/`diff_pair_uncoupled` constraint
    /// measures for two parallel-routed differential-pair tracks. `None`
    /// if the two segments aren't exactly parallel (checked via the exact
    /// integer cross product of their direction vectors — no float
    /// involved in *that* decision) or either is degenerate (zero
    /// length) — a caller with non-parallel or degenerate tracks has no
    /// coupling to report, not a coupling of zero.
    ///
    /// **Reporting-only, like [`Segment::length_nm`]**: every distance
    /// here goes through `f64::sqrt`/division, unlike this type's exact
    /// predicates. Returns *centreline* distances — `tessera-drc` is
    /// where a caller subtracts each track's own half-width to get the
    /// copper-edge gap real `.kicad_dru` `diff_pair_gap` bounds are
    /// checked against (the same net-width-aware split
    /// `clearance.rs::check_track_track` already uses for ordinary
    /// clearance).
    ///
    /// Verified against real `kicad-cli` on two parallel tracks (a
    /// 0.3mm centre-to-centre offset, 0.2mm wide each): reports a
    /// `diff_pair_gap` `actual` of 0.1mm — centreline distance minus
    /// both half-widths — and, for a second fixture where one track
    /// stops short of its partner, a `diff_pair_uncoupled` `actual`
    /// equal to exactly the excess length of the longer track beyond
    /// where the shorter one ends. See `docs/DECISIONS.md`'s
    /// "`diff_pair_gap`/`diff_pair_uncoupled`..." entry for the fixtures.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn diff_pair_coupling(self, other: Self) -> Option<DiffPairCoupling> {
        let d = self.b.sub(self.a);
        let d_other = other.b.sub(other.a);
        if d.length_sq() == 0 || d_other.length_sq() == 0 {
            return None;
        }
        if d.cross(d_other) != 0 {
            return None; // not parallel
        }

        #[allow(clippy::cast_precision_loss)]
        let self_len = (d.length_sq() as f64).sqrt();
        #[allow(clippy::cast_precision_loss)]
        let other_len = (d_other.length_sq() as f64).sqrt();

        // Perpendicular centreline distance: |cross(d, other.a - self.a)| / |d|.
        let ap: Vector = other.a.sub(self.a);
        #[allow(clippy::cast_precision_loss)]
        let cross_f64 = d.cross(ap) as f64;
        let gap = (cross_f64.abs() / self_len).round();

        // Project other's endpoints onto self's own direction (0..=self_len),
        // and overlap that interval with self's own [0, self_len] range —
        // the coupled run's length along the shared direction.
        #[allow(clippy::cast_precision_loss)]
        let project = |p: Point| -> f64 { p.sub(self.a).dot(d) as f64 / self_len };
        let (t_lo, t_hi) = {
            let (ta, tb) = (project(other.a), project(other.b));
            if ta <= tb {
                (ta, tb)
            } else {
                (tb, ta)
            }
        };
        let overlap_lo = t_lo.max(0.0);
        let overlap_hi = t_hi.min(self_len);
        let coupled = (overlap_hi - overlap_lo).max(0.0);
        let uncoupled = (self_len - coupled).max(0.0) + (other_len - coupled).max(0.0);

        Some(DiffPairCoupling {
            gap_nm: gap as i64,
            coupled_length_nm: coupled.round() as i64,
            uncoupled_length_nm: uncoupled.round() as i64,
        })
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
    fn length_nm_matches_pythagoras() {
        let seg = Segment::new(Point::new(0, 0), Point::new(3000, 4000));
        assert_eq!(seg.length_nm(), 5000);
    }

    #[test]
    fn length_nm_is_zero_for_degenerate_segment() {
        let seg = Segment::new(Point::new(5, 5), Point::new(5, 5));
        assert_eq!(seg.length_nm(), 0);
    }

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

    #[test]
    fn diff_pair_coupling_is_none_for_non_parallel_segments() {
        let a = Segment::new(Point::new(0, 0), Point::new(10, 0));
        let b = Segment::new(Point::new(0, 0), Point::new(0, 10));
        assert!(a.diff_pair_coupling(b).is_none());
    }

    #[test]
    fn diff_pair_coupling_is_none_for_a_degenerate_segment() {
        let a = Segment::new(Point::new(0, 0), Point::new(10, 0));
        let degenerate = Segment::new(Point::new(3, 3), Point::new(3, 3));
        assert!(a.diff_pair_coupling(degenerate).is_none());
    }

    #[test]
    fn diff_pair_coupling_matches_kicad_cli_on_two_fully_overlapping_parallel_tracks() {
        // Verified against real kicad-cli: two 0.2mm-wide tracks 0.3mm
        // apart (centre to centre), same 5mm span — diff_pair_gap actual
        // 0.1mm (centreline 0.3mm minus both 0.1mm half-widths, applied
        // at the tessera-drc layer, not here).
        let a = Segment::new(Point::new(0, 0), Point::new(5_000_000, 0));
        let b = Segment::new(Point::new(0, 300_000), Point::new(5_000_000, 300_000));
        let coupling = a.diff_pair_coupling(b).expect("parallel segments");
        assert_eq!(coupling.gap_nm, 300_000);
        assert_eq!(coupling.coupled_length_nm, 5_000_000);
        assert_eq!(coupling.uncoupled_length_nm, 0);
    }

    #[test]
    fn diff_pair_coupling_matches_kicad_cli_on_a_partially_overlapping_pair() {
        // Verified against real kicad-cli: SIG_P spans 5mm, SIG_N spans
        // only the first 3mm of it — diff_pair_uncoupled actual 2.0mm
        // (SIG_P's excess beyond where SIG_N ends).
        let a = Segment::new(Point::new(0, 0), Point::new(5_000_000, 0));
        let b = Segment::new(Point::new(0, 300_000), Point::new(3_000_000, 300_000));
        let coupling = a.diff_pair_coupling(b).expect("parallel segments");
        assert_eq!(coupling.gap_nm, 300_000);
        assert_eq!(coupling.coupled_length_nm, 3_000_000);
        assert_eq!(coupling.uncoupled_length_nm, 2_000_000);
    }

    #[test]
    fn diff_pair_coupling_is_symmetric_and_handles_reversed_direction() {
        // b declared start-to-end in the opposite direction of a — the
        // coupling must not depend on which endpoint is "start".
        let a = Segment::new(Point::new(0, 0), Point::new(5_000_000, 0));
        let b_reversed = Segment::new(Point::new(3_000_000, 300_000), Point::new(0, 300_000));
        let coupling = a.diff_pair_coupling(b_reversed).expect("parallel segments");
        assert_eq!(coupling.gap_nm, 300_000);
        assert_eq!(coupling.coupled_length_nm, 3_000_000);
        assert_eq!(coupling.uncoupled_length_nm, 2_000_000);
    }

    #[test]
    fn diff_pair_coupling_is_zero_when_projections_do_not_overlap_at_all() {
        let a = Segment::new(Point::new(0, 0), Point::new(2_000_000, 0));
        let b = Segment::new(
            Point::new(3_000_000, 300_000),
            Point::new(5_000_000, 300_000),
        );
        let coupling = a.diff_pair_coupling(b).expect("parallel segments");
        assert_eq!(coupling.coupled_length_nm, 0);
        assert_eq!(coupling.uncoupled_length_nm, 4_000_000); // both full lengths
    }
}
