//! Builders for the S3 calls bucket administration needs.
//!
//! Paths are URI-encoded once here (path-style addressing, `/bucket/key`), and
//! [`S3Request::url_path_and_query`] yields the exact string that is both signed
//! and sent, so the gateway sees the same canonical form the signature covers.
//! There is deliberately no object-upload builder: object management is out of
//! scope.

use super::sigv4::{canonical_query, uri_encode};

/// One S3 API call, ready to be signed and sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Request {
    /// HTTP method.
    pub method: &'static str,
    /// The URI-encoded request path, e.g. `/media/x%20y`.
    pub path: String,
    /// Unencoded query parameters.
    pub query: Vec<(String, String)>,
    /// The request body.
    pub body: Vec<u8>,
}

impl S3Request {
    fn new(method: &'static str, path: String) -> Self {
        Self {
            method,
            path,
            query: Vec::new(),
            body: Vec::new(),
        }
    }

    /// The encoded path plus the canonical query (when non-empty), as sent.
    #[must_use]
    pub fn url_path_and_query(&self) -> String {
        let query = canonical_query(&self.query);
        if query.is_empty() {
            self.path.clone()
        } else {
            format!("{}?{query}", self.path)
        }
    }
}

fn bucket_path(bucket: &str) -> String {
    format!("/{}", uri_encode(bucket, true))
}

/// `ListBuckets`: `GET /`.
#[must_use]
pub fn list_buckets() -> S3Request {
    S3Request::new("GET", "/".to_owned())
}

/// `CreateBucket`: `PUT /<bucket>` with an empty body (the gateway's own region).
#[must_use]
pub fn create_bucket(bucket: &str) -> S3Request {
    S3Request::new("PUT", bucket_path(bucket))
}

/// `DeleteBucket`: `DELETE /<bucket>`.
#[must_use]
pub fn delete_bucket(bucket: &str) -> S3Request {
    S3Request::new("DELETE", bucket_path(bucket))
}

/// `ListObjectsV2`: `GET /<bucket>?list-type=2&max-keys=<n>[&continuation-token=<t>]`.
#[must_use]
pub fn list_objects_v2(bucket: &str, continuation: Option<&str>, max_keys: u32) -> S3Request {
    let mut req = S3Request::new("GET", bucket_path(bucket));
    req.query.push(("list-type".to_owned(), "2".to_owned()));
    req.query
        .push(("max-keys".to_owned(), max_keys.to_string()));
    if let Some(token) = continuation {
        req.query
            .push(("continuation-token".to_owned(), token.to_owned()));
    }
    req
}

/// `DeleteObject`: `DELETE /<bucket>/<key>` (slashes in the key are kept).
#[must_use]
pub fn delete_object(bucket: &str, key: &str) -> S3Request {
    S3Request::new(
        "DELETE",
        format!("{}/{}", bucket_path(bucket), uri_encode(key, false)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_produce_expected_method_and_target() {
        let cases = [
            (list_buckets(), "GET", "/"),
            (create_bucket("media"), "PUT", "/media"),
            (delete_bucket("media"), "DELETE", "/media"),
            (delete_bucket("a/b"), "DELETE", "/a%2Fb"),
            (
                list_objects_v2("media", None, 1000),
                "GET",
                "/media?list-type=2&max-keys=1000",
            ),
            (
                list_objects_v2("media", Some("x y/ü"), 2),
                "GET",
                "/media?continuation-token=x%20y%2F%C3%BC&list-type=2&max-keys=2",
            ),
            (
                delete_object("media", "x y/ü.txt"),
                "DELETE",
                "/media/x%20y/%C3%BC.txt",
            ),
            (delete_object("media", "a&b"), "DELETE", "/media/a%26b"),
        ];
        for (req, method, target) in cases {
            assert_eq!(req.method, method, "{req:?}");
            assert_eq!(req.url_path_and_query(), target, "{req:?}");
            assert!(req.body.is_empty(), "{req:?}");
        }
    }
}
