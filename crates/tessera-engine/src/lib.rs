#![forbid(unsafe_code)]

mod negotiate;
mod ripup;
mod route;

pub use route::{route_board, RouteReport};
