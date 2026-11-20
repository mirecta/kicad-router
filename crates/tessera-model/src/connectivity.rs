//! Which copper of a net is already electrically joined: pads, tracks,
//! vias and zone fills that touch on a shared layer form one island.
//! [`crate::Board::find_unrouted_connections`] uses it to route only what's
//! still missing on a partly routed net, instead of all or nothing.

use tessera_geom::{Point, Polygon, Rect, Segment};

use crate::board::Board;
use crate::connection::Endpoint;
use crate::layer::LayerId;
use crate::net::NetId;
use crate::pad::PadShape;

/// A piece of copper reduced to a core shape grown by `radius`.
#[derive(Debug, Clone)]
enum Core<'a> {
    Point(Point),
    Segment(Segment),
    Rect(Rect),
    Polygon(&'a Polygon),
}

#[derive(Debug, Clone)]
struct Item<'a> {
    core: Core<'a>,
    radius: i64,
    layers: Vec<LayerId>,
    /// Bounding box including `radius`.
    min: Point,
    max: Point,
    /// Where a new route may start or end on this item, if anywhere
    /// (not on a fill: there's no single good spot on a pour).
    anchors: Vec<Endpoint>,
    is_pad: bool,
    /// Index into `Board::zone_fills`, for a fill.
    fill: Option<usize>,
}

impl<'a> Item<'a> {
    fn new(core: Core<'a>, radius: i64, layers: Vec<LayerId>) -> Self {
        let (min, max) = match &core {
            Core::Point(p) => (*p, *p),
            Core::Segment(s) => (
                Point::new(s.a.x.min(s.b.x), s.a.y.min(s.b.y)),
                Point::new(s.a.x.max(s.b.x), s.a.y.max(s.b.y)),
            ),
            Core::Rect(r) => (
                Point::new(r.center.x - r.half_width_nm, r.center.y - r.half_height_nm),
                Point::new(r.center.x + r.half_width_nm, r.center.y + r.half_height_nm),
            ),
            Core::Polygon(p) => p.bounding_box(0),
        };
        Self {
            core,
            radius,
            layers,
            min: Point::new(min.x - radius, min.y - radius),
            max: Point::new(max.x + radius, max.y + radius),
            anchors: Vec::new(),
            is_pad: false,
            fill: None,
        }
    }

    fn touches(&self, other: &Self) -> bool {
        if self.min.x > other.max.x
            || other.min.x > self.max.x
            || self.min.y > other.max.y
            || other.min.y > self.max.y
            || !self.layers.iter().any(|l| other.layers.contains(l))
        {
            return false;
        }
        // "clears" means at least `gap` apart; copper whose grown shapes
        // meet or overlap is joined (1 nm slack for rounding).
        let gap = self.radius + other.radius + 1;
        !clears(&self.core, &other.core, gap)
    }
}

fn clears(a: &Core, b: &Core, gap: i64) -> bool {
    use Core::{Point as P, Polygon as G, Rect as R, Segment as S};
    match (a, b) {
        (P(p), P(q)) => Segment::new(*p, *p).clears_point(*q, gap),
        (P(p), S(s)) | (S(s), P(p)) => s.clears_point(*p, gap),
        (P(p), R(r)) | (R(r), P(p)) => r.clears_point(*p, gap),
        (P(p), G(g)) | (G(g), P(p)) => g.clears_point(*p, gap),
        (S(s), S(t)) => s.clears_segment(*t, gap),
        (S(s), R(r)) | (R(r), S(s)) => r.clears_segment(*s, gap),
        (S(s), G(g)) | (G(g), S(s)) => g.clears_segment(*s, gap),
        (R(r), R(q)) => r.clears_rect(*q, gap),
        (R(r), G(g)) | (G(g), R(r)) => g.clears_rect(*r, gap),
        (G(g), G(h)) => g.clears_polygon(h, gap),
    }
}

/// One island of joined copper on a net that contains at least one pad,
/// with the spots a new route may connect to it.
#[derive(Debug, Clone)]
pub struct Island {
    pub anchors: Vec<Endpoint>,
}

/// The islands of `net` that hold a pad, in a deterministic order. Copper
/// without a pad (a stray track, a pour island over nothing) isn't
/// returned: it doesn't need connecting.
#[must_use]
pub fn pad_islands(board: &Board, net: NetId) -> Vec<Island> {
    let (items, mut parent) = joined_items(board, net);
    let mut islands: Vec<(usize, Island, bool)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let r = root(&mut parent, i);
        let idx = islands
            .iter()
            .position(|(k, _, _)| *k == r)
            .unwrap_or_else(|| {
                islands.push((
                    r,
                    Island {
                        anchors: Vec::new(),
                    },
                    false,
                ));
                islands.len() - 1
            });
        islands[idx].1.anchors.extend(item.anchors.iter().cloned());
        islands[idx].2 |= item.is_pad;
    }
    islands
        .into_iter()
        .filter(|(_, _, has_pad)| *has_pad)
        .map(|(_, island, _)| island)
        .collect()
}

/// The zone fills (indices into `Board::zone_fills`) of `net` joined to the
/// copper at `at` (a pad centre, a track end, a via) — the pour a stitching
/// via must reach to join that copper.
#[must_use]
pub fn fills_joined_to(board: &Board, net: NetId, at: Point) -> Vec<usize> {
    let (items, mut parent) = joined_items(board, net);
    let Some(start) = items
        .iter()
        .position(|it| it.anchors.iter().any(|a| a.position == at))
    else {
        return Vec::new();
    };
    let r = root(&mut parent, start);
    (0..items.len())
        .filter(|&i| root(&mut parent, i) == r)
        .filter_map(|i| items[i].fill)
        .collect()
}

/// The zone fills (indices into `Board::zone_fills`) of `net`, grouped by
/// the island of copper they belong to, each with whether that island
/// holds a pad. A pour island without one is the "isolated copper" KiCad's
/// DRC warns about. Islands without any fill aren't returned.
#[must_use]
pub fn fill_islands(board: &Board, net: NetId) -> Vec<(Vec<usize>, bool)> {
    let (items, mut parent) = joined_items(board, net);
    let mut islands: Vec<(usize, Vec<usize>, bool)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let r = root(&mut parent, i);
        let idx = islands
            .iter()
            .position(|(k, _, _)| *k == r)
            .unwrap_or_else(|| {
                islands.push((r, Vec::new(), false));
                islands.len() - 1
            });
        islands[idx].1.extend(item.fill);
        islands[idx].2 |= item.is_pad;
    }
    islands
        .into_iter()
        .filter(|(_, fills, _)| !fills.is_empty())
        .map(|(_, fills, has_pad)| (fills, has_pad))
        .collect()
}

/// The zone fills (indices into `Board::zone_fills`) of `net` that touch
/// nothing but one through-hole pad, which on that layer reaches no other
/// fill: KiCad counts such an island as isolated, so the pad's thermal
/// spokes into it as not connected (`starved_thermal`). A pour under one
/// SMD pad alone (a switch node's) is a design, not a cut-off island.
#[must_use]
pub fn fills_on_one_pad(board: &Board, net: NetId) -> Vec<usize> {
    let (items, _) = joined_items(board, net);
    let touching = |fill: &Item| -> Vec<usize> {
        (0..items.len())
            .filter(|&i| items[i].fill.is_none() && items[i].touches(fill))
            .collect()
    };
    // per fill: the one through-hole pad it hangs on, if that's all it touches
    let hung: Vec<(usize, usize, LayerId)> = items
        .iter()
        .filter_map(|fill| {
            let k = fill.fill?;
            match touching(fill)[..] {
                [pad] if items[pad].is_pad && items[pad].layers.len() > 1 => {
                    Some((k, pad, *fill.layers.first()?))
                }
                _ => None,
            }
        })
        .collect();
    hung.iter()
        .filter(|&&(_, pad, layer)| {
            // every fill the pad reaches on this layer hangs on it alone
            items.iter().filter(|it| it.fill.is_some()).all(|fill| {
                !(fill.layers.contains(&layer) && fill.touches(&items[pad]))
                    || hung
                        .iter()
                        .any(|&(k, p, _)| Some(k) == fill.fill && p == pad)
            })
        })
        .map(|&(k, _, _)| k)
        .collect()
}

/// `net`'s copper as items, and a union-find over those that touch.
fn joined_items(board: &Board, net: NetId) -> (Vec<Item<'_>>, Vec<usize>) {
    let all_layers: Vec<LayerId> = board.layers.iter().map(|l| l.id).collect();
    let mut items: Vec<Item> = Vec::new();
    for pad in board.pads.iter().filter(|p| p.net == net) {
        let mut item = match &pad.shape {
            PadShape::Circle(c) => {
                Item::new(Core::Point(c.center), c.radius_nm, pad.layers.clone())
            }
            PadShape::Rect(r) => Item::new(Core::Rect(*r), 0, pad.layers.clone()),
            PadShape::Oval(o) => Item::new(Core::Segment(o.spine), o.radius_nm, pad.layers.clone()),
            PadShape::Polygon { polygon, .. } => {
                Item::new(Core::Polygon(polygon), 0, pad.layers.clone())
            }
        };
        item.anchors.push(Endpoint {
            position: pad.shape.center(),
            layers: pad.layers.clone(),
        });
        item.is_pad = true;
        items.push(item);
    }
    for track in board.tracks.iter().filter(|t| t.net == net) {
        let mut item = Item::new(
            Core::Segment(track.segment),
            track.width_nm / 2,
            vec![track.layer],
        );
        for position in [track.segment.a, track.segment.b] {
            item.anchors.push(Endpoint {
                position,
                layers: vec![track.layer],
            });
        }
        items.push(item);
    }
    for via in board.vias.iter().filter(|v| v.net == net) {
        let (lo, hi) = (
            via.from_layer.0.min(via.to_layer.0),
            via.from_layer.0.max(via.to_layer.0),
        );
        let layers: Vec<LayerId> = all_layers
            .iter()
            .copied()
            .filter(|l| (lo..=hi).contains(&l.0))
            .collect();
        let mut item = Item::new(
            Core::Point(via.position),
            via.diameter_nm / 2,
            layers.clone(),
        );
        item.anchors.push(Endpoint {
            position: via.position,
            layers,
        });
        items.push(item);
    }
    for (index, fill) in board.zone_fills.iter().enumerate() {
        if fill.net != net {
            continue;
        }
        let mut item = Item::new(
            Core::Polygon(&fill.polygon),
            fill.inflation_nm,
            vec![fill.layer],
        );
        item.fill = Some(index);
        items.push(item);
    }

    // union-find over touching items
    let mut parent: Vec<usize> = (0..items.len()).collect();
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            if root(&mut parent, i) != root(&mut parent, j) && items[i].touches(&items[j]) {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[b] = a;
            }
        }
    }
    (items, parent)
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// The links that join `islands` into one, as a minimum spanning tree over
/// the islands, each link between the nearest pair of their anchors.
#[must_use]
pub fn links_between(islands: &[Island]) -> Vec<(Endpoint, Endpoint)> {
    let n = islands.len();
    if n < 2 {
        return Vec::new();
    }
    let dist = |a: Point, b: Point| (i128::from(a.x - b.x)).pow(2) + (i128::from(a.y - b.y)).pow(2);
    let nearest = |i: usize, j: usize| {
        let mut best: Option<(i128, usize, usize)> = None;
        for (ai, a) in islands[i].anchors.iter().enumerate() {
            for (bi, b) in islands[j].anchors.iter().enumerate() {
                let d = dist(a.position, b.position);
                if best.is_none_or(|(bd, _, _)| d < bd) {
                    best = Some((d, ai, bi));
                }
            }
        }
        best
    };
    let table: Vec<Vec<Option<(i128, usize, usize)>>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| if i < j { nearest(i, j) } else { None })
                .collect()
        })
        .collect();
    let lookup = |i: usize, j: usize| {
        if i < j {
            table[i][j]
        } else {
            table[j][i].map(|(d, a, b)| (d, b, a))
        }
    };
    // Prim's algorithm
    let mut in_tree = vec![false; n];
    in_tree[0] = true;
    let mut links = Vec::new();
    for _ in 1..n {
        let mut best: Option<(i128, usize, usize, usize, usize)> = None;
        for i in (0..n).filter(|&i| in_tree[i]) {
            for j in (0..n).filter(|&j| !in_tree[j]) {
                if let Some((d, ai, bi)) = lookup(i, j) {
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, i, j, ai, bi));
                    }
                }
            }
        }
        let Some((_, i, j, ai, bi)) = best else { break };
        in_tree[j] = true;
        links.push((
            islands[i].anchors[ai].clone(),
            islands[j].anchors[bi].clone(),
        ));
    }
    links
}
