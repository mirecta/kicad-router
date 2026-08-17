use crate::circle::Circle;
use crate::point::Point;
use crate::rect::Rect;
use crate::segment::Segment;

/// An oval (KiCad's `oval` pad shape) — a stadium/discorectangle: a
/// straight `spine` swept by a constant `radius_nm`, i.e. the Minkowski
/// sum of a line segment and a circle. **Verified against real
/// `kicad-cli`, not assumed**: a diagonal point placed exactly on the
/// hypothesised circular-cap boundary (a point at distance
/// `spine_end + radius` from the spine's end, off-axis, not just along
/// the long axis where a stadium and an ellipse would agree anyway)
/// reports the exact clearance a constant-radius circular cap predicts
/// — ruling out an ellipse, which would give a different, smaller
/// effective radius off-axis. See `docs/DECISIONS.md`'s oval-pad entry
/// for the fixture.
///
/// This shape is exactly how a track's own clearance envelope already
/// works in this crate (`Segment` + `width_nm`, e.g.
/// `clearance.rs::check_track_track`'s `effective_min = min_nm +
/// a.width_nm / 2 + b.width_nm / 2`) — an oval pad is a "thick segment"
/// in precisely the same sense a track is, which is why every method
/// below is a thin wrapper around `Segment`'s already-exact predicates
/// rather than new geometry: no new clearance math was needed here,
/// only the empirical confirmation that KiCad's oval really is this
/// shape (not an ellipse) before relying on that reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Oval {
    /// The oval's central axis. Degenerate (`a == b`) when `width_nm ==
    /// height_nm` — [`Segment::distance_sq`] already handles a
    /// degenerate segment as a plain point, so a "square" oval reduces
    /// to a circle automatically, no special-casing needed here.
    pub spine: Segment,
    /// Half of the oval's *shorter* dimension — the sweep radius.
    pub radius_nm: i64,
    pub center: Point,
}

impl Oval {
    /// Builds an oval centred at `center` from its axis-aligned
    /// `(width_nm, height_nm)` — the spine runs along whichever
    /// dimension is longer (ties go to a horizontal spine, matching a
    /// zero-length spine's direction being irrelevant once
    /// `width_nm == height_nm` makes this a circle anyway).
    #[must_use]
    pub fn new(center: Point, width_nm: i64, height_nm: i64) -> Self {
        debug_assert!(width_nm >= 0 && height_nm >= 0);
        let spine = if width_nm >= height_nm {
            let half_spine = (width_nm - height_nm) / 2;
            Segment::new(
                Point::new(center.x - half_spine, center.y),
                Point::new(center.x + half_spine, center.y),
            )
        } else {
            let half_spine = (height_nm - width_nm) / 2;
            Segment::new(
                Point::new(center.x, center.y - half_spine),
                Point::new(center.x, center.y + half_spine),
            )
        };
        let radius_nm = width_nm.min(height_nm) / 2;
        Self {
            spine,
            radius_nm,
            center,
        }
    }

    #[must_use]
    pub fn clears_point(self, point: Point, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        self.spine
            .clears_point(point, min_nm.saturating_add(self.radius_nm))
    }

    #[must_use]
    pub fn clears_circle(self, circle: Circle, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        let threshold = min_nm
            .saturating_add(self.radius_nm)
            .saturating_add(circle.radius_nm);
        self.spine.clears_point(circle.center, threshold)
    }

    #[must_use]
    pub fn clears_segment(self, segment: Segment, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        self.spine
            .clears_segment(segment, min_nm.saturating_add(self.radius_nm))
    }

    #[must_use]
    pub fn clears_rect(self, rect: Rect, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        rect.clears_segment(self.spine, min_nm.saturating_add(self.radius_nm))
    }

    #[must_use]
    pub fn clears_oval(self, other: Self, min_nm: i64) -> bool {
        debug_assert!(min_nm >= 0);
        let threshold = min_nm
            .saturating_add(self.radius_nm)
            .saturating_add(other.radius_nm);
        self.spine.clears_segment(other.spine, threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_oval_reduces_to_a_circle() {
        let o = Oval::new(Point::new(0, 0), 1000, 1000);
        assert_eq!(o.spine.a, o.spine.b);
        assert_eq!(o.radius_nm, 500);
        assert!(o.clears_point(Point::new(600, 0), 100));
        assert!(!o.clears_point(Point::new(600, 0), 101));
    }

    #[test]
    fn spine_runs_along_the_longer_axis() {
        let wide = Oval::new(Point::new(0, 0), 2000, 1000);
        assert_eq!(wide.spine.a, Point::new(-500, 0));
        assert_eq!(wide.spine.b, Point::new(500, 0));
        assert_eq!(wide.radius_nm, 500);

        let tall = Oval::new(Point::new(0, 0), 1000, 2000);
        assert_eq!(tall.spine.a, Point::new(0, -500));
        assert_eq!(tall.spine.b, Point::new(0, 500));
        assert_eq!(tall.radius_nm, 500);
    }

    #[test]
    fn diagonal_cap_clearance_matches_kicad_cli() {
        // Verified against real kicad-cli (see docs/DECISIONS.md's
        // oval-pad entry): a diagonal marker placed off-axis from a 2x1mm
        // oval's end-cap reports exactly the clearance a constant-radius
        // circular cap predicts, ruling out an ellipse. This test
        // reproduces the same geometry with exact (not rounded-mm)
        // coordinates: a 3-4-5 triangle scaled by 120_000, so the marker
        // sits exactly 600_000nm from the spine's end.
        let oval = Oval::new(Point::new(0, 0), 2_000_000, 1_000_000); // spine end at (500_000, 0), radius 500_000
        let marker = Point::new(500_000 + 480_000, 360_000); // dx=480_000, dy=360_000 -> distance 600_000 exactly
                                                             // gap to the oval's own boundary is 600_000 - 500_000 = 100_000.
        assert!(oval.clears_point(marker, 100_000));
        assert!(!oval.clears_point(marker, 100_001));
    }

    #[test]
    fn clears_rect_matches_a_thick_segment_against_the_rect() {
        let oval = Oval::new(Point::new(0, 0), 2_000_000, 1_000_000); // radius 500_000
        let rect = Rect::new(Point::new(2_000_000, 0), 1_000_000, 1_000_000); // half-width 500_000
                                                                              // spine end at (500_000,0); rect left edge at 1_500_000: gap
                                                                              // between spine end and rect edge is 1_000_000, minus the
                                                                              // oval's own 500_000 radius = 500_000.
        assert!(oval.clears_rect(rect, 500_000));
        assert!(!oval.clears_rect(rect, 500_001));
    }

    #[test]
    fn clears_oval_sums_both_radii() {
        let a = Oval::new(Point::new(0, 0), 2_000_000, 1_000_000); // radius 500_000, spine end at 500_000
        let b = Oval::new(Point::new(3_000_000, 0), 1_000_000, 2_000_000); // radius 500_000, spine along Y
                                                                           // a's spine end (500_000,0); b's spine is the vertical segment
                                                                           // at x=3_000_000 (from y=-500_000 to 500_000) — closest point on
                                                                           // b's spine to a's spine end is (3_000_000, 0), distance
                                                                           // 2_500_000; minus both radii (500_000 each) = 1_500_000.
        assert!(a.clears_oval(b, 1_500_000));
        assert!(!a.clears_oval(b, 1_500_001));
    }
}
