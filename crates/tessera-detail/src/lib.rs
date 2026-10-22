#![forbid(unsafe_code)]

mod astar;
mod congestion;
mod grid;
mod obstacle;
mod router;
mod smooth;

pub use congestion::Congestion;
pub use grid::{Cell, CorridorMask, GridBounds, ObstacleMap, CELL_NM};
pub use obstacle::{
    obstacles_from_board, Blocks, Obstacle, ObstacleClearance, ObstacleKind, ObstacleShape,
};
pub use router::{
    fanout_to_plane, path_is_legal, route_connection, route_connection_with_cost, RoutedPath,
};
