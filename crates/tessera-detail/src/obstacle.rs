use tessera_geom::{Circle, Point, Segment};
use tessera_model::{Board, LayerId, NetId, PadShape};

/// Whether an obstacle may ever be considered for rip-up by a future
/// scheduler (plan §5.4, M5). `Frozen` obstacles are structurally excluded
/// from that possibility: this is a distinct enum variant, not a
/// `locked: bool` flag a rip-up candidate-selection routine could
/// accidentally ignore. Per plan §7.5.4, "enforce it in the type system if
/// you can" — there is (deliberately) no rip-up API in this crate yet at
/// all, so there's nothing to accidentally accept a `Frozen` obstacle;
/// when M5 adds one, it should take `&[Obstacle]` filtered to `Movable`
/// only, or a type that makes accepting a `Frozen` one a compile error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObstacleKind {
    Movable,
    Frozen,
}

#[derive(Debug, Clone, Copy)]
pub enum ObstacleShape {
    Circle(Circle),
    /// A track segment plus its width — the swept clearance shape, not the
    /// zero-width line `Segment` alone represents (mirrors the convention
    /// established in `tessera-drc::clearance`).
    Segment(Segment, i64),
}

impl ObstacleShape {
    /// True iff `point` is at least `min_nm` from this shape's copper.
    #[must_use]
    pub fn clears_point(self, point: Point, min_nm: i64) -> bool {
        match self {
            ObstacleShape::Circle(c) => c.clears_point(point, min_nm),
            ObstacleShape::Segment(s, width_nm) => s.clears_point(point, min_nm + width_nm / 2),
        }
    }

    /// An axis-aligned bounding box, expanded by `margin_nm`, as
    /// `(min, max)` points — used to limit which grid cells a caller needs
    /// to test against this obstacle at all.
    #[must_use]
    pub fn bounding_box(self, margin_nm: i64) -> (Point, Point) {
        match self {
            ObstacleShape::Circle(c) => (
                Point::new(
                    c.center.x - c.radius_nm - margin_nm,
                    c.center.y - c.radius_nm - margin_nm,
                ),
                Point::new(
                    c.center.x + c.radius_nm + margin_nm,
                    c.center.y + c.radius_nm + margin_nm,
                ),
            ),
            ObstacleShape::Segment(s, width_nm) => {
                let half = width_nm / 2 + margin_nm;
                let min_x = s.a.x.min(s.b.x) - half;
                let max_x = s.a.x.max(s.b.x) + half;
                let min_y = s.a.y.min(s.b.y) - half;
                let max_y = s.a.y.max(s.b.y) + half;
                (Point::new(min_x, min_y), Point::new(max_x, max_y))
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Obstacle {
    pub shape: ObstacleShape,
    pub layer: LayerId,
    pub net: NetId,
    pub kind: ObstacleKind,
}

fn kind_of(locked: bool) -> ObstacleKind {
    if locked {
        ObstacleKind::Frozen
    } else {
        ObstacleKind::Movable
    }
}

fn layers_between(from: LayerId, to: LayerId) -> impl Iterator<Item = LayerId> {
    let lo = from.0.min(to.0);
    let hi = from.0.max(to.0);
    (lo..=hi).map(LayerId)
}

/// Builds one [`Obstacle`] per copper layer each board item occupies — a
/// through via spanning F.Cu..B.Cu becomes two entries, not one
/// multi-layer entry, so every consumer's per-layer collision check stays
/// uniform (a track only ever needs to check its own single layer).
#[must_use]
pub fn obstacles_from_board(board: &Board) -> Vec<Obstacle> {
    let mut obstacles = Vec::new();

    for track in &board.tracks {
        obstacles.push(Obstacle {
            shape: ObstacleShape::Segment(track.segment, track.width_nm),
            layer: track.layer,
            net: track.net,
            kind: kind_of(track.locked),
        });
    }

    for via in &board.vias {
        let circle = Circle::new(via.position, via.diameter_nm / 2);
        for layer in layers_between(via.from_layer, via.to_layer) {
            obstacles.push(Obstacle {
                shape: ObstacleShape::Circle(circle),
                layer,
                net: via.net,
                kind: kind_of(via.locked),
            });
        }
    }

    for pad in &board.pads {
        let PadShape::Circle(circle) = &pad.shape;
        for &layer in &pad.layers {
            obstacles.push(Obstacle {
                shape: ObstacleShape::Circle(*circle),
                layer,
                net: pad.net,
                kind: kind_of(pad.locked),
            });
        }
    }

    obstacles
}
