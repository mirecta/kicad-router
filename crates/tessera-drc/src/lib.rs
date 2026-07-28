#![forbid(unsafe_code)]

mod clearance;
mod violation;

pub use clearance::{check_clearance, resolved_clearance_nm};
pub use violation::{ClearanceViolation, ItemRef};
