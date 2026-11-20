#![forbid(unsafe_code)]

mod negotiate;
mod pair;
mod ripup;
mod route;
mod stitching;

pub use route::{route_board, RouteReport};
