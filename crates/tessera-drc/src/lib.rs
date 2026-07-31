#![forbid(unsafe_code)]

mod clearance;
mod custom_rules;
mod violation;

pub use clearance::{check_clearance, resolved_clearance_nm};
pub use custom_rules::{
    check_disallow, check_track_width, Bound, CustomRuleViolation, DisallowViolation,
};
pub use violation::{ClearanceViolation, ItemRef};
