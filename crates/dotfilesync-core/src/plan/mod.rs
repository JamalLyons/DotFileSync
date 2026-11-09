//! Desired-state plan. The plan body is deterministic: no clocks and no random ids.

mod model;
mod planner;

pub use model::*;
pub use planner::run_plan;
