#![forbid(unsafe_code)]

mod astar;
mod grid;
mod obstacle;
mod router;

pub use grid::{Cell, GridBounds, ObstacleMap, CELL_NM};
pub use obstacle::{obstacles_from_board, Obstacle, ObstacleKind, ObstacleShape};
pub use router::{route_connection, RoutedPath};
