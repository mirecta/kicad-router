use tessera_geom::Point;
use tessera_model::Endpoint;

/// One edge of a decomposed multi-pin net: a two-pin connection between
/// two of the net's endpoints, suitable for `tessera-detail::route_connection`
/// exactly as an ordinary two-pin net would be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteinerEdge {
    pub from: Endpoint,
    pub to: Endpoint,
}

fn manhattan_distance(a: Point, b: Point) -> i64 {
    (a.x - b.x).abs() + (a.y - b.y).abs()
}

/// Decomposes a multi-pin net into a set of two-pin edges connecting every
/// endpoint, via a minimum rectilinear spanning tree (Prim's algorithm over
/// Manhattan distance).
///
/// This is a standard, simple approximation to the rectilinear Steiner
/// minimal tree problem — no Steiner points are added, only direct
/// pin-to-pin edges, so total length is longer than the true optimum by at
/// most a well-known constant factor (the "Steiner ratio," classically
/// bounded at 3/2 for rectilinear MST-based heuristics). Plan §5.1 names
/// **FLUTE** (Chu & Wong) as the eventual target, which does add Steiner
/// points and gets closer to optimal — but FLUTE is itself a port
/// candidate (plan §11.5) and porting it requires the formal human-gated
/// procedure in plan §11.3 (read → prose explanation → human review →
/// independent design → implement → differential test) before any of that
/// code gets written. This function is an original, from-first-principles
/// implementation of a textbook algorithm, not a port of anything, so it
/// doesn't need that gate — it exists to unblock M2's explicit gap
/// (`Board::find_unrouted_connections` reports 3+-pin nets as skipped
/// rather than routing them) with something real now, upgradeable to FLUTE
/// later without changing this function's callers (`Vec<SteinerEdge>` in,
/// `tessera-detail::route_connection`-shaped edges out either way).
///
/// Returns `endpoints.len().saturating_sub(1)` edges connecting every
/// endpoint into a single tree; an empty or single-endpoint input returns
/// no edges (nothing to connect).
#[must_use]
pub fn minimum_spanning_tree(endpoints: &[Endpoint]) -> Vec<SteinerEdge> {
    let n = endpoints.len();
    if n < 2 {
        return Vec::new();
    }

    let mut in_tree = vec![false; n];
    let mut best_distance = vec![i64::MAX; n];
    let mut best_from = vec![0usize; n];

    in_tree[0] = true;
    for (j, endpoint) in endpoints.iter().enumerate().skip(1) {
        best_distance[j] = manhattan_distance(endpoints[0].position, endpoint.position);
    }

    let mut edges = Vec::with_capacity(n - 1);
    for _ in 1..n {
        let Some(next) = (0..n)
            .filter(|&j| !in_tree[j])
            .min_by_key(|&j| best_distance[j])
        else {
            break; // unreachable for a complete graph over a non-empty point set
        };

        in_tree[next] = true;
        edges.push(SteinerEdge {
            from: endpoints[best_from[next]].clone(),
            to: endpoints[next].clone(),
        });

        for k in 0..n {
            if !in_tree[k] {
                let distance = manhattan_distance(endpoints[next].position, endpoints[k].position);
                if distance < best_distance[k] {
                    best_distance[k] = distance;
                    best_from[k] = next;
                }
            }
        }
    }

    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_model::LayerId;

    fn endpoint(x: i64, y: i64) -> Endpoint {
        Endpoint {
            position: Point::new(x, y),
            layers: vec![LayerId(0)],
        }
    }

    #[test]
    fn empty_and_single_endpoint_produce_no_edges() {
        assert!(minimum_spanning_tree(&[]).is_empty());
        assert!(minimum_spanning_tree(&[endpoint(0, 0)]).is_empty());
    }

    #[test]
    fn two_endpoints_produce_one_edge() {
        let edges = minimum_spanning_tree(&[endpoint(0, 0), endpoint(1_000_000, 0)]);
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn picks_shortest_connections_over_a_longer_direct_one() {
        // Three collinear points: middle one should link both neighbours
        // (two short edges) rather than the two outer points linking
        // directly across the middle (one long edge) plus the middle
        // dangling — a spanning tree always has exactly n-1 edges, but
        // this checks *which* n-1 edges: the cheap ones.
        let a = endpoint(0, 0);
        let b = endpoint(1_000_000, 0);
        let c = endpoint(3_000_000, 0);
        let edges = minimum_spanning_tree(&[a.clone(), b.clone(), c.clone()]);
        assert_eq!(edges.len(), 2);

        let connects = |edges: &[SteinerEdge], p: &Endpoint, q: &Endpoint| {
            edges
                .iter()
                .any(|e| (e.from == *p && e.to == *q) || (e.from == *q && e.to == *p))
        };
        assert!(connects(&edges, &a, &b), "a-b is the cheapest edge at a");
        assert!(connects(&edges, &b, &c), "b-c is cheaper than a-c");
        assert!(
            !connects(&edges, &a, &c),
            "a-c would be the redundant long edge"
        );
    }

    #[test]
    fn always_produces_exactly_n_minus_one_edges_and_stays_connected() {
        let points = [
            (0, 0),
            (5_000_000, 0),
            (2_000_000, 3_000_000),
            (-4_000_000, 1_000_000),
            (1_000_000, -2_000_000),
        ];
        let endpoints: Vec<Endpoint> = points.iter().map(|&(x, y)| endpoint(x, y)).collect();
        let edges = minimum_spanning_tree(&endpoints);
        assert_eq!(edges.len(), endpoints.len() - 1);
        assert!(is_connected(&endpoints, &edges));
    }

    fn is_connected(endpoints: &[Endpoint], edges: &[SteinerEdge]) -> bool {
        if endpoints.is_empty() {
            return true;
        }
        let mut visited = vec![false; endpoints.len()];
        let mut stack = vec![0usize];
        visited[0] = true;
        while let Some(i) = stack.pop() {
            for edge in edges {
                for (a, b) in [(&edge.from, &edge.to), (&edge.to, &edge.from)] {
                    if *a == endpoints[i] {
                        if let Some(j) = endpoints.iter().position(|e| e == b) {
                            if !visited[j] {
                                visited[j] = true;
                                stack.push(j);
                            }
                        }
                    }
                }
            }
        }
        visited.iter().all(|&v| v)
    }
}
