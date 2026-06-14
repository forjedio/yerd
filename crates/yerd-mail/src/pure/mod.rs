//! Pure, side-effect-free logic: SMTP command handling, MIME decoding, and the
//! retention policy. Everything here is sync and unit-testable without sockets
//! or a filesystem.

pub mod mime;
pub mod retention;
pub mod smtp;
