#![forbid(unsafe_code)]
// NOTE: this crate will eventually FFI into Clipper2 for polygon boolean/offset
// (plan §4.2). When that lands, narrow this to `#![deny(unsafe_code)]` and
// document each `unsafe` block's invariant at the FFI boundary (plan §12).

mod circle;
mod point;
mod polygon;
mod predicates;
mod segment;

pub use circle::Circle;
pub use point::{Point, Vector, MAX_COORDINATE_NM};
pub use polygon::Polygon;
pub use predicates::{orient, Orientation};
pub use segment::{RationalDistanceSq, Segment};
