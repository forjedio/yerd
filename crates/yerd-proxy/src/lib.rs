//! HTTP/HTTPS reverse proxy for Yerd's `*.test` traffic.
//!
//! See `@docs/ARCHITECTURE.md` §6.6 for the high-level design.

#![forbid(unsafe_code)]
// Domain shorthand like `FastCGI`, `FrankenPHP`, `BEGIN_REQUEST`, etc. is
// pervasive in proxy docs; backticking every occurrence adds noise.
#![allow(clippy::doc_markdown)]

pub mod backend;
pub mod error;
pub mod forward;
pub mod pure;
pub mod server;
pub mod tls;
pub mod traits;

pub use backend::Backend;
pub use error::ProxyError;
pub use server::{HttpsBinding, ProxyServer};
pub use traits::{BackendResolver, CertStore};

// Compile-time guard: ProxyError must stay Send+Sync+'static so it can
// cross hyper service boundaries and tokio::spawn sites cleanly.
const _: () = {
    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<ProxyError>();
};
