//! Authoritative *.test DNS responder for Yerd.

#![forbid(unsafe_code)]

mod answer;
mod error;
mod responder;
mod server;

pub use answer::Answer;
pub use error::{BindProto, DnsError};
pub use responder::Responder;
pub use server::Bound;

/// TTL on every A/AAAA record we hand out.
pub const ANSWER_TTL_SECS: u32 = 60;

/// Number of UDP/TCP port-pair attempts on the ephemeral path.
pub(crate) const RETRY_BUDGET: usize = 5;
