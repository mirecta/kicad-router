use std::sync::Arc;

use tessera_geom::{Circle, Oval, Point, Polygon, Rect, Segment};
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

#[derive(Debug, Clone)]
pub enum ObstacleShape {
    Circle(Circle),
    /// A track segment plus its width — the swept clearance shape, not the
    /// zero-width line `Segment` alone represents (mirrors the convention
    /// established in `tessera-drc::clearance`).
    Segment(Segment, i64),
    /// A non-circular pad's copper shape (2026-08-17, once `tessera-geom`
    /// gained exact `Rect` clearance predicates — see that type's docs
    /// for the still-real limitations, e.g. no footprint rotation).
    Rect(Rect),
    /// A stadium-shaped pad's copper shape (2026-08-17, once
    /// `tessera-geom` gained `Oval` — see that type's docs for why it's
    /// built entirely on `Segment`'s existing predicates).
    Oval(Oval),
    /// An existing copper-zone fill (shared, since the same fill may be
    /// cloned into several search windows) and how far KiCad grows it on
    /// load (`ZoneFill::inflation_nm`).
    Polygon(Arc<Polygon>, i64),
}

/// Which kind of new copper an obstacle keeps out — a keepout area may
/// forbid only tracks, or only vias.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocks {
    Both,
    TracksOnly,
    ViasOnly,
}

/// How an obstacle's required clearance is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObstacleClearance {
    /// Resolved from the two nets' classes.
    NetClass,
    /// A fixed board rule regardless of nets (a non-plated hole's
    /// `min_hole_clearance`).
    Fixed(i64),
    /// The net-class clearance, but never less than this (a zone fill's
    /// own clearance — see `Board::zone_clearance_nm`).
    AtLeast(i64),
}

impl ObstacleShape {
    /// True iff `point` is at least `min_nm` from this shape's copper.
    #[must_use]
    pub fn clears_point(&self, point: Point, min_nm: i64) -> bool {
        match self {
            ObstacleShape::Circle(c) => c.clears_point(point, min_nm),
            ObstacleShape::Segment(s, width_nm) => s.clears_point(point, min_nm + width_nm / 2),
            ObstacleShape::Rect(r) => r.clears_point(point, min_nm),
            ObstacleShape::Oval(o) => o.clears_point(point, min_nm),
            ObstacleShape::Polygon(p, inflation_nm) => p.clears_point(point, min_nm + inflation_nm),
        }
    }

    /// True iff every point of `segment` is at least `min_nm` from this
    /// shape's copper — the exact check path straightening relies on.
    #[must_use]
    pub fn clears_segment(&self, segment: Segment, min_nm: i64) -> bool {
        match self {
            ObstacleShape::Circle(c) => c.clears_segment(segment, min_nm),
            ObstacleShape::Segment(s, width_nm) => s.clears_segment(segment, min_nm + width_nm / 2),
            ObstacleShape::Rect(r) => r.clears_segment(segment, min_nm),
            ObstacleShape::Oval(o) => o.clears_segment(segment, min_nm),
            ObstacleShape::Polygon(p, inflation_nm) => {
                p.clears_segment(segment, min_nm + inflation_nm)
            }
        }
    }

    /// An axis-aligned bounding box, expanded by `margin_nm`, as
    /// `(min, max)` points — used to limit which grid cells a caller needs
    /// to test against this obstacle at all.
    #[must_use]
    pub fn bounding_box(&self, margin_nm: i64) -> (Point, Point) {
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
            ObstacleShape::Rect(r) => (
                Point::new(
                    r.center.x - r.half_width_nm - margin_nm,
                    r.center.y - r.half_height_nm - margin_nm,
                ),
                Point::new(
                    r.center.x + r.half_width_nm + margin_nm,
                    r.center.y + r.half_height_nm + margin_nm,
                ),
            ),
            ObstacleShape::Oval(o) => {
                let half = o.radius_nm + margin_nm;
                let min_x = o.spine.a.x.min(o.spine.b.x) - half;
                let max_x = o.spine.a.x.max(o.spine.b.x) + half;
                let min_y = o.spine.a.y.min(o.spine.b.y) - half;
                let max_y = o.spine.a.y.max(o.spine.b.y) + half;
                (Point::new(min_x, min_y), Point::new(max_x, max_y))
            }
            ObstacleShape::Polygon(p, inflation_nm) => p.bounding_box(margin_nm + inflation_nm),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Obstacle {
    pub shape: ObstacleShape,
    pub layer: LayerId,
    pub net: NetId,
    pub kind: ObstacleKind,
    pub clearance: ObstacleClearance,
    pub blocks: Blocks,
}

impl Obstacle {
    /// The copper clearance a new track or via on `routed_net` must keep
    /// from this obstacle, or `None` if it's on that same net. Fails
    /// closed: an unresolvable net class (a malformed board) gets a
    /// generous 0.5mm rather than silently allowing contact.
    #[must_use]
    pub fn required_clearance_nm(&self, board: &Board, routed_net: NetId) -> Option<i64> {
        if self.net.same_net(routed_net) {
            return None;
        }
        let net_class_nm = board.resolved_clearance_nm(self.net, routed_net);
        let clearance_nm = match self.clearance {
            ObstacleClearance::NetClass => net_class_nm,
            ObstacleClearance::Fixed(nm) => Some(nm),
            ObstacleClearance::AtLeast(nm) => net_class_nm.map(|c| c.max(nm)),
        };
        Some(clearance_nm.unwrap_or(500_000))
    }
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
            clearance: ObstacleClearance::NetClass,
            blocks: Blocks::Both,
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
                clearance: ObstacleClearance::NetClass,
                blocks: Blocks::Both,
            });
        }
    }

    for pad in &board.pads {
        let shape = match &pad.shape {
            PadShape::Circle(c) => ObstacleShape::Circle(*c),
            PadShape::Rect(r) => ObstacleShape::Rect(*r),
            PadShape::Oval(o) => ObstacleShape::Oval(*o),
        };
        for &layer in &pad.layers {
            obstacles.push(Obstacle {
                shape: shape.clone(),
                layer,
                net: pad.net,
                kind: kind_of(pad.locked),
                clearance: ObstacleClearance::NetClass,
                blocks: Blocks::Both,
            });
        }
    }

    for fill in &board.zone_fills {
        obstacles.push(Obstacle {
            shape: ObstacleShape::Polygon(Arc::new(fill.polygon.clone()), fill.inflation_nm),
            layer: fill.layer,
            net: fill.net,
            kind: ObstacleKind::Frozen,
            clearance: ObstacleClearance::AtLeast(fill.clearance_nm),
            blocks: Blocks::Both,
        });
    }

    // The board edge: every outline edge is a wall new copper keeps
    // `min_copper_edge_clearance` from (verified against real `kicad-cli`:
    // measured to the edge line, regardless of net). Together the edges
    // close the board, so nothing can be routed off it either.
    if let Some(outline) = &board.outline {
        let n = outline.vertices.len();
        for i in 0..n {
            let edge = Segment::new(outline.vertices[i], outline.vertices[(i + 1) % n]);
            for layer in &board.layers {
                obstacles.push(Obstacle {
                    shape: ObstacleShape::Segment(edge, 0),
                    layer: layer.id,
                    net: NetId::NONE,
                    kind: ObstacleKind::Frozen,
                    clearance: ObstacleClearance::Fixed(board.rules.min_copper_edge_clearance_nm),
                    blocks: Blocks::Both,
                });
            }
        }
    }

    // Keepout rule areas: verified against real `kicad-cli` that new copper
    // of a forbidden kind may not *overlap* the area on its layers (no
    // clearance beyond that).
    for area in &board.rule_areas {
        let blocks = match (area.keepout.tracks_allowed, area.keepout.vias_allowed) {
            (true, true) => continue,
            (false, false) => Blocks::Both,
            (false, true) => Blocks::TracksOnly,
            (true, false) => Blocks::ViasOnly,
        };
        let polygon = Arc::new(area.outline.clone());
        for &layer in &area.layers {
            obstacles.push(Obstacle {
                shape: ObstacleShape::Polygon(Arc::clone(&polygon), 0),
                layer,
                net: NetId::NONE,
                kind: ObstacleKind::Frozen,
                clearance: ObstacleClearance::Fixed(0),
                blocks,
            });
        }
    }

    for hole in &board.npth_holes {
        for layer in &board.layers {
            obstacles.push(Obstacle {
                shape: ObstacleShape::Circle(hole.circle()),
                layer: layer.id,
                net: NetId::NONE,
                kind: ObstacleKind::Frozen,
                clearance: ObstacleClearance::Fixed(board.rules.min_hole_clearance_nm),
                blocks: Blocks::Both,
            });
        }
    }

    obstacles
}
