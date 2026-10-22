use tessera_geom::{Point, Segment};
use tessera_model::LayerId;

/// An extra cost the search adds for passing through a cell — never
/// negative, so the A* heuristic stays admissible.
pub trait CostField {
    /// The extra cost at `p` on the `layer_index`-th signal layer (in
    /// `Board::signal_layers` order).
    fn cost(&self, p: Point, layer_index: usize) -> i64;
}

/// Extra cost per step for each 0.1mm a route strays from the coupling
/// distance, and the most a single step can cost — paid wherever the
/// partner isn't nearby on the same layer at all.
const COUPLING_SLOPE: i64 = 200;
const COUPLING_CAP: i64 = 1000;

/// Keeps a diff pair's second member beside the first: free at exactly
/// `distance_nm` (centre to centre) from the partner's tracks on the same
/// layer, dearer the further off, capped where the partner isn't near.
pub struct Coupling {
    partner: Vec<(Segment, LayerId)>,
    layers: Vec<LayerId>,
    distance_nm: i64,
}

impl Coupling {
    #[must_use]
    pub fn new(partner: Vec<(Segment, LayerId)>, layers: Vec<LayerId>, distance_nm: i64) -> Self {
        Self {
            partner,
            layers,
            distance_nm,
        }
    }
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn distance_nm(p: Point, s: Segment) -> i64 {
    let (ax, ay) = (s.a.x as f64, s.a.y as f64);
    let (dx, dy) = (s.b.x as f64 - ax, s.b.y as f64 - ay);
    let (px, py) = (p.x as f64 - ax, p.y as f64 - ay);
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq == 0.0 {
        0.0
    } else {
        ((px * dx + py * dy) / len_sq).clamp(0.0, 1.0)
    };
    ((px - t * dx).hypot(py - t * dy)).round() as i64
}

impl CostField for Coupling {
    fn cost(&self, p: Point, layer_index: usize) -> i64 {
        let Some(&layer) = self.layers.get(layer_index) else {
            return COUPLING_CAP;
        };
        self.partner
            .iter()
            .filter(|(_, l)| *l == layer)
            .map(|(s, _)| distance_nm(p, *s))
            .min()
            .map_or(COUPLING_CAP, |d| {
                ((d - self.distance_nm).abs() * COUPLING_SLOPE / 100_000).min(COUPLING_CAP)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coupling_is_free_at_the_distance_and_capped_far_away_or_off_layer() {
        let partner = vec![(
            Segment::new(Point::new(0, 0), Point::new(10_000_000, 0)),
            LayerId(0),
        )];
        let c = Coupling::new(partner, vec![LayerId(0), LayerId(1)], 450_000);
        assert_eq!(c.cost(Point::new(5_000_000, 450_000), 0), 0);
        assert_eq!(c.cost(Point::new(5_000_000, 550_000), 0), COUPLING_SLOPE);
        assert_eq!(c.cost(Point::new(5_000_000, 5_000_000), 0), COUPLING_CAP);
        assert_eq!(c.cost(Point::new(5_000_000, 450_000), 1), COUPLING_CAP);
    }
}
