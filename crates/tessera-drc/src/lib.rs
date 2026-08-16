#![forbid(unsafe_code)]

mod clearance;
mod custom_rules;
mod violation;

pub use clearance::{check_clearance, resolved_clearance_nm};
pub use custom_rules::{
    check_custom_clearance, check_disallow, check_length, check_track_width, Bound,
    CustomClearanceViolation, CustomRuleViolation, DisallowViolation, LengthViolation,
};
pub use violation::{ClearanceViolation, ItemRef};
