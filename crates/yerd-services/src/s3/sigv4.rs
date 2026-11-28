//! AWS Signature Version 4 for S3, header-based, single-chunk payload.
//!
//! Follows the S3 variant of the algorithm: the canonical URI is the request path
//! exactly as sent (already URI-encoded once, never normalised or encoded again),
//! and the signed headers are always `host;x-amz-content-sha256;x-amz-date`.
//! Checked against the published AWS S3 examples in the tests below.

use std::fmt::Write as _;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::credentials::RootCredentials;

type HmacSha256 = Hmac<Sha256>;

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const SERVICE: &str = "s3";
const SIGNED_HEADERS: &str = "host;x-amz-content-sha256;x-amz-date";

/// A Unix timestamp that cannot be represented as a calendar date.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("timestamp {0} is out of range for a SigV4 date")]
pub struct AmzDateError(pub u64);

/// The request time in the two forms Signature Version 4 uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmzDate {
    date: String,
    datetime: String,
}

impl AmzDate {
    /// Build from seconds since the Unix epoch (UTC).
    ///
    /// # Errors
    /// [`AmzDateError`] when the timestamp is outside the supported calendar range.
    pub fn from_unix(secs: u64) -> Result<Self, AmzDateError> {
        let stamp = i64::try_from(secs).map_err(|_| AmzDateError(secs))?;
        let t = time::OffsetDateTime::from_unix_timestamp(stamp).map_err(|_| AmzDateError(secs))?;
        let date = format!("{:04}{:02}{:02}", t.year(), u8::from(t.month()), t.day());
        let datetime = format!("{date}T{:02}{:02}{:02}Z", t.hour(), t.minute(), t.second());
        Ok(Self { date, datetime })
    }

    /// `YYYYMMDD`, used in the credential scope.
    #[must_use]
    pub fn date(&self) -> &str {
        &self.date
    }

    /// `YYYYMMDDTHHMMSSZ`, sent as `x-amz-date`.
    #[must_use]
    pub fn datetime(&self) -> &str {
        &self.datetime
    }
}

/// The request parts that are signed.
#[derive(Debug, Clone, Copy)]
pub struct SigningInput<'a> {
    /// HTTP method, e.g. `GET`.
    pub method: &'a str,
    /// The encoded request path exactly as sent, e.g. `/bucket/key`.
    pub canonical_uri: &'a str,
    /// Unencoded query parameters; encoded and sorted by [`canonical_query`].
    pub query: &'a [(String, String)],
    /// The `Host` header value, e.g. `127.0.0.1:7070`.
    pub host: &'a str,
    /// Lowercase hex SHA-256 of the request body.
    pub payload_sha256_hex: &'a str,
}

/// The headers a signed request must carry.
#[derive(Clone, PartialEq, Eq)]
pub struct SignedHeaders {
    /// The `Authorization` header value.
    pub authorization: String,
    /// The `x-amz-date` header value.
    pub x_amz_date: String,
    /// The `x-amz-content-sha256` header value.
    pub x_amz_content_sha256: String,
}

impl std::fmt::Debug for SignedHeaders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedHeaders")
            .field("authorization", &"<redacted>")
            .field("x_amz_date", &self.x_amz_date)
            .field("x_amz_content_sha256", &self.x_amz_content_sha256)
            .finish()
    }
}

/// S3 URI encoding: RFC 3986 unreserved characters (`A-Za-z0-9-._~`) are kept,
/// every other UTF-8 byte becomes `%XX` (uppercase hex). `/` is kept unless
/// `encode_slash` is set.
#[must_use]
pub fn uri_encode(s: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~');
        if unreserved || (b == b'/' && !encode_slash) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// The canonical query string: each key and value URI-encoded (slashes too),
/// sorted by key then value, joined as `k=v` with `&` (`k=` for empty values).
#[must_use]
pub fn canonical_query(query: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| (uri_encode(k, true), uri_encode(v, true)))
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Lowercase hex SHA-256 of `data`.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// The Signature Version 4 canonical request for `input` at `date`.
#[must_use]
pub fn canonical_request(input: &SigningInput<'_>, date: &AmzDate) -> String {
    format!(
        "{}\n{}\n{}\nhost:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n\n{SIGNED_HEADERS}\n{}",
        input.method,
        input.canonical_uri,
        canonical_query(input.query),
        input.host,
        input.payload_sha256_hex,
        date.datetime(),
        input.payload_sha256_hex,
    )
}

fn scope(date: &AmzDate, region: &str) -> String {
    format!("{}/{region}/{SERVICE}/aws4_request", date.date())
}

/// The Signature Version 4 string to sign for `input` at `date` in `region`.
#[must_use]
pub fn string_to_sign(input: &SigningInput<'_>, region: &str, date: &AmzDate) -> String {
    format!(
        "{ALGORITHM}\n{}\n{}\n{}",
        date.datetime(),
        scope(date, region),
        sha256_hex(canonical_request(input, date).as_bytes())
    )
}

/// HMAC-SHA256 of `data` under `key`.
///
/// HMAC accepts keys of any length, so `new_from_slice` cannot fail here; the
/// empty fallback only exists to stay total, and would surface as the gateway's
/// own `SignatureDoesNotMatch` rather than a panic.
fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    match <HmacSha256 as Mac>::new_from_slice(key) {
        Ok(mut mac) => {
            mac.update(data);
            mac.finalize().into_bytes().to_vec()
        }
        Err(_) => Vec::new(),
    }
}

/// Sign `input` with `creds` for `region` at `date`.
#[must_use]
pub fn sign(
    input: &SigningInput<'_>,
    creds: &RootCredentials,
    region: &str,
    date: &AmzDate,
) -> SignedHeaders {
    let k_date = hmac(
        format!("AWS4{}", creds.secret_key).as_bytes(),
        date.date().as_bytes(),
    );
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, SERVICE.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(
        &k_signing,
        string_to_sign(input, region, date).as_bytes(),
    ));
    SignedHeaders {
        authorization: format!(
            "{ALGORITHM} Credential={}/{},SignedHeaders={SIGNED_HEADERS},Signature={signature}",
            creds.access_key,
            scope(date, region)
        ),
        x_amz_date: date.datetime().to_owned(),
        x_amz_content_sha256: input.payload_sha256_hex.to_owned(),
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const AWS_EXAMPLE_UNIX: u64 = 1_369_353_600;
    const AWS_HOST: &str = "examplebucket.s3.amazonaws.com";

    fn aws_creds() -> RootCredentials {
        RootCredentials {
            access_key: "AKIAIOSFODNN7EXAMPLE".to_owned(),
            secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_owned(),
        }
    }

    fn q(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    struct AwsExample {
        name: &'static str,
        query: Vec<(String, String)>,
        canonical_request: &'static str,
        string_to_sign: &'static str,
        signature: &'static str,
    }

    fn aws_examples() -> Vec<AwsExample> {
        vec![
            AwsExample {
                name: "GET Bucket Lifecycle",
                query: q(&[("lifecycle", "")]),
                canonical_request: "GET\n/\nlifecycle=\nhost:examplebucket.s3.amazonaws.com\nx-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nx-amz-date:20130524T000000Z\n\nhost;x-amz-content-sha256;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                string_to_sign: "AWS4-HMAC-SHA256\n20130524T000000Z\n20130524/us-east-1/s3/aws4_request\n9766c798316ff2757b517bc739a67f6213b4ab36dd5da2f94eaebf79c77395ca",
                signature: "fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543",
            },
            AwsExample {
                name: "GET Bucket (List Objects)",
                query: q(&[("prefix", "J"), ("max-keys", "2")]),
                canonical_request: "GET\n/\nmax-keys=2&prefix=J\nhost:examplebucket.s3.amazonaws.com\nx-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nx-amz-date:20130524T000000Z\n\nhost;x-amz-content-sha256;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                string_to_sign: "AWS4-HMAC-SHA256\n20130524T000000Z\n20130524/us-east-1/s3/aws4_request\ndf57d21db20da04d7fa30298dd4488ba3a2b47ca3a489c74750e0f1e7df1b9b7",
                signature: "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7",
            },
        ]
    }

    #[test]
    fn matches_aws_s3_single_chunk_examples() {
        let date = AmzDate::from_unix(AWS_EXAMPLE_UNIX).unwrap_or_else(|e| panic!("{e}"));
        for ex in aws_examples() {
            let input = SigningInput {
                method: "GET",
                canonical_uri: "/",
                query: &ex.query,
                host: AWS_HOST,
                payload_sha256_hex: EMPTY_SHA256,
            };
            assert_eq!(
                canonical_request(&input, &date),
                ex.canonical_request,
                "{}",
                ex.name
            );
            assert_eq!(
                string_to_sign(&input, "us-east-1", &date),
                ex.string_to_sign,
                "{}",
                ex.name
            );
            let signed = sign(&input, &aws_creds(), "us-east-1", &date);
            assert_eq!(
                signed.authorization,
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,SignedHeaders=host;x-amz-content-sha256;x-amz-date,Signature={}",
                    ex.signature
                ),
                "{}",
                ex.name
            );
            assert_eq!(signed.x_amz_date, "20130524T000000Z");
            assert_eq!(signed.x_amz_content_sha256, EMPTY_SHA256);
        }
    }

    #[test]
    fn signed_headers_debug_redacts_authorization() {
        let date = AmzDate::from_unix(AWS_EXAMPLE_UNIX).unwrap_or_else(|e| panic!("{e}"));
        let input = SigningInput {
            method: "GET",
            canonical_uri: "/",
            query: &[],
            host: AWS_HOST,
            payload_sha256_hex: EMPTY_SHA256,
        };
        let signed = sign(&input, &aws_creds(), "us-east-1", &date);
        let shown = format!("{signed:?}");
        assert!(!shown.contains("Signature="), "{shown}");
        assert!(!shown.contains("AKIAIOSFODNN7EXAMPLE"), "{shown}");
    }

    #[test]
    fn uri_encode_table() {
        let cases = [
            ("abc-._~XYZ019", false, "abc-._~XYZ019"),
            ("a b", false, "a%20b"),
            ("a/b", false, "a/b"),
            ("a/b", true, "a%2Fb"),
            ("~", true, "~"),
            ("ü", false, "%C3%BC"),
            ("+", false, "%2B"),
            ("a&b=c", true, "a%26b%3Dc"),
            ("*", false, "%2A"),
            ("x y/ü.txt", false, "x%20y/%C3%BC.txt"),
            ("", true, ""),
        ];
        for (input, encode_slash, want) in cases {
            assert_eq!(
                uri_encode(input, encode_slash),
                want,
                "{input:?} encode_slash={encode_slash}"
            );
        }
    }

    #[test]
    fn canonical_query_table() {
        let cases = [
            (q(&[]), ""),
            (q(&[("lifecycle", "")]), "lifecycle="),
            (
                q(&[("prefix", "J"), ("max-keys", "2")]),
                "max-keys=2&prefix=J",
            ),
            (
                q(&[
                    ("max-keys", "1000"),
                    ("list-type", "2"),
                    ("continuation-token", "a b/c+="),
                ]),
                "continuation-token=a%20b%2Fc%2B%3D&list-type=2&max-keys=1000",
            ),
            (q(&[("k", "b"), ("k", "a")]), "k=a&k=b"),
        ];
        for (query, want) in cases {
            assert_eq!(canonical_query(&query), want, "{query:?}");
        }
    }

    #[test]
    fn amz_date_from_aws_example() {
        let date = AmzDate::from_unix(AWS_EXAMPLE_UNIX).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(date.date(), "20130524");
        assert_eq!(date.datetime(), "20130524T000000Z");
    }

    #[test]
    fn amz_date_rejects_out_of_range() {
        assert_eq!(AmzDate::from_unix(u64::MAX), Err(AmzDateError(u64::MAX)));
    }

    #[test]
    fn sha256_hex_of_empty_input() {
        assert_eq!(sha256_hex(b""), EMPTY_SHA256);
    }
}
