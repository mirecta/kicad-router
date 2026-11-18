#![forbid(unsafe_code)]

mod negotiate;
mod pair;
mod ripup;
mod route;

pub use route::{route_board, RouteReport};
