//! Pure S3 administration logic for the local versitygw gateway.
//!
//! No I/O: bucket-name validation, AWS Signature Version 4 signing, request
//! construction, XML reply parsing and error classification. The daemon's
//! `s3_admin` edge sends the requests built here over loopback HTTP and feeds the
//! replies back through [`xml`] and [`classify`]. Requests are signed for
//! [`crate::service::S3_REGION`], the region the gateway is launched with.

pub mod bucket_name;
pub mod request;
pub mod sigv4;
pub mod xml;

pub use bucket_name::{validate_bucket_name, BucketNameError};
pub use request::S3Request;
pub use sigv4::{AmzDate, AmzDateError, SignedHeaders, SigningInput};
pub use xml::{ObjectsPage, S3ErrorBody, S3XmlError};

/// The service id of the only S3 engine Yerd manages.
pub const SERVICE_ID: &str = "versitygw";

/// The S3 error codes the admin layer branches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3Failure {
    /// The bucket already exists (`BucketAlreadyExists` or `BucketAlreadyOwnedByYou`).
    AlreadyExists,
    /// The bucket does not exist (`NoSuchBucket`).
    NoSuchBucket,
    /// The bucket still holds objects (`BucketNotEmpty`).
    BucketNotEmpty,
    /// Any other S3 error code.
    Other,
}

/// Classify an S3 `<Code>` value.
#[must_use]
pub fn classify(code: &str) -> S3Failure {
    match code {
        "BucketAlreadyExists" | "BucketAlreadyOwnedByYou" => S3Failure::AlreadyExists,
        "NoSuchBucket" => S3Failure::NoSuchBucket,
        "BucketNotEmpty" => S3Failure::BucketNotEmpty,
        _ => S3Failure::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_maps_known_codes() {
        let cases = [
            ("BucketAlreadyExists", S3Failure::AlreadyExists),
            ("BucketAlreadyOwnedByYou", S3Failure::AlreadyExists),
            ("NoSuchBucket", S3Failure::NoSuchBucket),
            ("BucketNotEmpty", S3Failure::BucketNotEmpty),
            ("AccessDenied", S3Failure::Other),
            ("nosuchbucket", S3Failure::Other),
            ("", S3Failure::Other),
        ];
        for (code, want) in cases {
            assert_eq!(classify(code), want, "code {code:?}");
        }
    }
}
