//! Per-backend forwarding I/O.

pub mod fcgi;
pub mod http;
pub mod static_file;
pub mod upgrade;

/// Body type used in proxy responses - boxed so all forward variants
/// (streaming FCGI STDOUT, streaming hyper Incoming, empty 101) can be
/// returned from the same `handle_request`.
pub type BoxBody = http_body_util::combinators::BoxBody<bytes::Bytes, std::io::Error>;

/// Build an empty `BoxBody` (for 301/404/501/101 responses).
pub fn empty_body() -> BoxBody {
    use http_body_util::BodyExt;
    http_body_util::Empty::<bytes::Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

/// Build a `BoxBody` from a static byte slice.
pub fn bytes_body(bytes: &'static [u8]) -> BoxBody {
    use http_body_util::BodyExt;
    http_body_util::Full::new(bytes::Bytes::from_static(bytes))
        .map_err(|never| match never {})
        .boxed()
}

/// Build a `BoxBody` from owned bytes (e.g. a rendered HTML page).
pub fn owned_bytes_body(bytes: Vec<u8>) -> BoxBody {
    use http_body_util::BodyExt;
    http_body_util::Full::new(bytes::Bytes::from(bytes))
        .map_err(|never| match never {})
        .boxed()
}
