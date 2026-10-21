#![forbid(unsafe_code)]

mod clearance;
mod custom_rules;
mod holes;
mod violation;

pub use clearance::{check_clearance, resolved_clearance_nm};
pub use custom_rules::{
    check_custom_clearance, check_diff_pair_gap, check_diff_pair_uncoupled, check_disallow,
    check_hole_size, check_length, check_track_width, check_via_diameter, Bound,
    CustomClearanceViolation, CustomRuleViolation, DiffPairViolation, DisallowViolation,
    LengthViolation,
};
pub use holes::{check_hole_clearance, HoleClearanceViolation};
pub use violation::{ClearanceViolation, ItemRef};
