//! Pure, side-effect-free Insights logic. Everything here is sync, clock-free
//! and unit-testable without a filesystem or sockets.

pub mod aggregator;
pub mod codec;
pub mod contract;
pub mod dict;
pub mod hist;
pub mod issues;
pub mod model;
mod num;
pub mod retention;
pub mod rollup;
pub mod route;
pub mod sql;
