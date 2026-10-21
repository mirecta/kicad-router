#![forbid(unsafe_code)]

mod astar;
mod grid;
mod obstacle;
mod router;
mod smooth;

pub use grid::{Cell, CorridorMask, GridBounds, ObstacleMap, CELL_NM};
pub use obstacle::{
    obstacles_from_board, Blocks, Obstacle, ObstacleClearance, ObstacleKind, ObstacleShape,
};
pub use router::{fanout_to_plane, route_connection, RoutedPath};
