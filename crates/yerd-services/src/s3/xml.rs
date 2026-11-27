//! Parsers for the S3 XML replies bucket administration reads.
//!
//! quick-xml 0.38 reports entity and character references (`&amp;`, `&#34;`)
//! as separate `GeneralRef` events between `Text` events, so element text is
//! rebuilt by concatenating every text, CDATA and resolved reference until the
//! element closes. Namespaces are ignored (local names only).

use quick_xml::events::{BytesRef, Event};
use quick_xml::Reader;
use thiserror::Error;

/// A reply that is not the XML shape expected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum S3XmlError {
    /// The document is not well-formed XML.
    #[error("malformed S3 XML reply: {0}")]
    Malformed(String),
    /// The root element is not the one this reply should have.
    #[error("unexpected S3 XML reply: expected <{expected}>, found <{found}>")]
    UnexpectedRoot {
        /// The expected root element.
        expected: &'static str,
        /// The root element found (empty when there was none).
        found: String,
    },
    /// A required element is missing.
    #[error("S3 XML reply is missing <{0}>")]
    MissingField(&'static str),
    /// A numeric element does not hold a number.
    #[error("S3 XML reply has a non-numeric <{field}>: {value:?}")]
    InvalidNumber {
        /// The element.
        field: &'static str,
        /// The text it held.
        value: String,
    },
}

/// One page of a `ListObjectsV2` reply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectsPage {
    /// `(key, size in bytes)` for each object on the page.
    pub objects: Vec<(String, u64)>,
    /// The token for the next page, when the listing is truncated.
    pub next_token: Option<String>,
}

/// The `<Code>` and `<Message>` of an S3 `<Error>` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3ErrorBody {
    /// The S3 error code, e.g. `NoSuchBucket`.
    pub code: String,
    /// The human-readable message (may be empty).
    pub message: String,
}

fn resolve_ref(r: &BytesRef<'_>) -> Result<String, S3XmlError> {
    let malformed = |e: &dyn std::fmt::Display| S3XmlError::Malformed(e.to_string());
    if let Some(c) = r.resolve_char_ref().map_err(|e| malformed(&e))? {
        return Ok(c.to_string());
    }
    let name = r.decode().map_err(|e| malformed(&e))?;
    quick_xml::escape::resolve_predefined_entity(&name)
        .map(str::to_owned)
        .ok_or_else(|| S3XmlError::Malformed(format!("unknown entity &{name};")))
}

/// Walk `xml`, calling `on_end(path, text)` as each element closes, where
/// `path` is the element names from the root down to the closing one and `text`
/// is its concatenated direct text. Returns the root element's name.
fn walk(
    xml: &str,
    mut on_end: impl FnMut(&[String], &str) -> Result<(), S3XmlError>,
) -> Result<String, S3XmlError> {
    let mut reader = Reader::from_str(xml);
    let mut path: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut root = String::new();
    let malformed = |e: &dyn std::fmt::Display| S3XmlError::Malformed(e.to_string());
    loop {
        match reader.read_event().map_err(|e| malformed(&e))? {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if path.is_empty() && root.is_empty() {
                    root.clone_from(&name);
                }
                path.push(name);
                texts.push(String::new());
            }
            Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if path.is_empty() && root.is_empty() {
                    root.clone_from(&name);
                }
                path.push(name);
                on_end(&path, "")?;
                path.pop();
            }
            Event::End(_) => {
                let text = texts.pop().unwrap_or_default();
                on_end(&path, &text)?;
                path.pop();
            }
            Event::Text(t) => {
                if let Some(buf) = texts.last_mut() {
                    buf.push_str(&t.decode().map_err(|e| malformed(&e))?);
                }
            }
            Event::CData(t) => {
                if let Some(buf) = texts.last_mut() {
                    buf.push_str(&t.decode().map_err(|e| malformed(&e))?);
                }
            }
            Event::GeneralRef(r) => {
                let resolved = resolve_ref(&r)?;
                if let Some(buf) = texts.last_mut() {
                    buf.push_str(&resolved);
                }
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
    if !path.is_empty() {
        return Err(S3XmlError::Malformed(format!(
            "unexpected end of document inside <{}>",
            path.join("/")
        )));
    }
    Ok(root)
}

fn ends_with(path: &[String], tail: &[&str]) -> bool {
    path.len() >= tail.len()
        && path
            .iter()
            .rev()
            .zip(tail.iter().rev())
            .all(|(a, b)| a == b)
}

fn expect_root(root: String, expected: &'static str) -> Result<(), S3XmlError> {
    if root == expected {
        Ok(())
    } else {
        Err(S3XmlError::UnexpectedRoot {
            expected,
            found: root,
        })
    }
}

/// Bucket names from a `ListAllMyBucketsResult` reply, in reply order.
///
/// # Errors
/// [`S3XmlError`] for malformed XML or a different root element.
pub fn parse_list_buckets(xml: &str) -> Result<Vec<String>, S3XmlError> {
    let mut names = Vec::new();
    let root = walk(xml, |path, text| {
        if path.len() == 4 && ends_with(path, &["Buckets", "Bucket", "Name"]) {
            names.push(text.to_owned());
        }
        Ok(())
    })?;
    expect_root(root, "ListAllMyBucketsResult")?;
    Ok(names)
}

/// One page of a `ListBucketResult` (`ListObjectsV2`) reply.
///
/// # Errors
/// [`S3XmlError`] for malformed XML, a different root element, an object
/// without `<Key>` or with a non-numeric `<Size>`, or a truncated page without
/// a `<NextContinuationToken>`.
pub fn parse_list_objects_v2(xml: &str) -> Result<ObjectsPage, S3XmlError> {
    let mut page = ObjectsPage::default();
    let mut key: Option<String> = None;
    let mut size: u64 = 0;
    let mut truncated = false;
    let mut token: Option<String> = None;
    let root = walk(xml, |path, text| {
        if path.len() == 3 && ends_with(path, &["Contents", "Key"]) {
            key = Some(text.to_owned());
        } else if path.len() == 3 && ends_with(path, &["Contents", "Size"]) {
            size = text.trim().parse().map_err(|_| S3XmlError::InvalidNumber {
                field: "Size",
                value: text.to_owned(),
            })?;
        } else if path.len() == 2 && ends_with(path, &["Contents"]) {
            let k = key.take().ok_or(S3XmlError::MissingField("Key"))?;
            page.objects.push((k, size));
            size = 0;
        } else if path.len() == 2 && ends_with(path, &["IsTruncated"]) {
            truncated = text.trim() == "true";
        } else if path.len() == 2 && ends_with(path, &["NextContinuationToken"]) {
            token = Some(text.to_owned()).filter(|t| !t.is_empty());
        }
        Ok(())
    })?;
    expect_root(root, "ListBucketResult")?;
    if truncated {
        page.next_token = Some(token.ok_or(S3XmlError::MissingField("NextContinuationToken"))?);
    }
    Ok(page)
}

/// The code and message of an `<Error>` reply.
///
/// # Errors
/// [`S3XmlError`] for malformed XML, a different root element, or a missing
/// `<Code>`.
pub fn parse_error(xml: &str) -> Result<S3ErrorBody, S3XmlError> {
    let mut code: Option<String> = None;
    let mut message = String::new();
    let root = walk(xml, |path, text| {
        if path.len() == 2 && ends_with(path, &["Code"]) {
            code = Some(text.to_owned());
        } else if path.len() == 2 && ends_with(path, &["Message"]) {
            text.clone_into(&mut message);
        }
        Ok(())
    })?;
    expect_root(root, "Error")?;
    let code = code
        .filter(|c| !c.is_empty())
        .ok_or(S3XmlError::MissingField("Code"))?;
    Ok(S3ErrorBody { code, message })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const LIST_BUCKETS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListAllMyBucketsResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Owner><ID>yerd_testkey01</ID><DisplayName></DisplayName></Owner><Buckets><Bucket><CreationDate>2026-11-27T22:07:17Z</CreationDate><Name>assets</Name><BucketRegion>us-east-1</BucketRegion></Bucket><Bucket><CreationDate>2026-11-27T22:07:17Z</CreationDate><Name>media</Name><BucketRegion>us-east-1</BucketRegion></Bucket></Buckets></ListAllMyBucketsResult>"#;

    const LIST_BUCKETS_EMPTY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListAllMyBucketsResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Owner><ID>yerd_testkey01</ID><DisplayName></DisplayName></Owner><Buckets></Buckets></ListAllMyBucketsResult>"#;

    const LIST_OBJECTS_TRUNCATED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>media</Name><NextContinuationToken>a1</NextContinuationToken><KeyCount>2</KeyCount><MaxKeys>2</MaxKeys><IsTruncated>true</IsTruncated><Contents><LastModified>2026-11-27T22:07:17Z</LastModified><ChecksumAlgorithm>CRC64NVME</ChecksumAlgorithm><ChecksumType>FULL_OBJECT</ChecksumType><ETag>&#34;5d41402abc4b2a76b9719d911017c592&#34;</ETag><Key>a&amp;b</Key><Size>5</Size><StorageClass>STANDARD</StorageClass></Contents><Contents><LastModified>2026-11-27T22:07:17Z</LastModified><ChecksumAlgorithm>CRC64NVME</ChecksumAlgorithm><ChecksumType>FULL_OBJECT</ChecksumType><ETag>&#34;5d41402abc4b2a76b9719d911017c592&#34;</ETag><Key>a1</Key><Size>5</Size><StorageClass>STANDARD</StorageClass></Contents><EncodingType></EncodingType></ListBucketResult>"#;

    const LIST_OBJECTS_FINAL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>media</Name><ContinuationToken>a1</ContinuationToken><KeyCount>2</KeyCount><MaxKeys>2</MaxKeys><IsTruncated>false</IsTruncated><Contents><LastModified>2026-11-27T22:07:17Z</LastModified><ChecksumAlgorithm>CRC64NVME</ChecksumAlgorithm><ChecksumType>FULL_OBJECT</ChecksumType><ETag>&#34;5d41402abc4b2a76b9719d911017c592&#34;</ETag><Key>b2</Key><Size>5</Size><StorageClass>STANDARD</StorageClass></Contents><Contents><LastModified>2026-11-27T22:07:17Z</LastModified><ChecksumAlgorithm>CRC64NVME</ChecksumAlgorithm><ChecksumType>FULL_OBJECT</ChecksumType><ETag>&#34;5d41402abc4b2a76b9719d911017c592&#34;</ETag><Key>x y/ü.txt</Key><Size>5</Size><StorageClass>STANDARD</StorageClass></Contents><EncodingType></EncodingType></ListBucketResult>"#;

    const LIST_OBJECTS_EMPTY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>assets</Name><KeyCount>0</KeyCount><MaxKeys>2</MaxKeys><IsTruncated>false</IsTruncated><EncodingType></EncodingType></ListBucketResult>"#;

    const ERROR_NO_SUCH_BUCKET: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Error><Code>NoSuchBucket</Code><Message>The specified bucket does not exist.</Message><BucketName>nope</BucketName><RequestId>179G57C8R08NG4YS</RequestId><HostId>LnrJpYeP8FKkDTa6F0iywwsTwSEwWpByi2aD5z/7Hkiedt3CCpmdX07pcEf+U0JK6sATgJ3eWvLJjlqRxIFA6hk=</HostId></Error>"#;

    const ERROR_NOT_EMPTY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Error><Code>BucketNotEmpty</Code><Message>The bucket you tried to delete is not empty.</Message><BucketName>media</BucketName><RequestId>PQOWEQH6G49WPVQ7</RequestId><HostId>kOFxwhzUSCimsxoEZZVaBpSGbA4n5CkMgxLfoxMzZ+FzI3Mw553s30Uj4s/qq67gt7fRXZWOaJWgv3Twy1rkiQg=</HostId></Error>"#;

    #[test]
    fn list_buckets_fixtures() {
        let cases: [(&str, Vec<&str>); 2] = [
            (LIST_BUCKETS, vec!["assets", "media"]),
            (LIST_BUCKETS_EMPTY, vec![]),
        ];
        for (xml, want) in cases {
            assert_eq!(parse_list_buckets(xml).unwrap(), want);
        }
    }

    #[test]
    fn list_objects_fixtures() {
        let cases = [
            (
                LIST_OBJECTS_TRUNCATED,
                ObjectsPage {
                    objects: vec![("a&b".to_owned(), 5), ("a1".to_owned(), 5)],
                    next_token: Some("a1".to_owned()),
                },
            ),
            (
                LIST_OBJECTS_FINAL,
                ObjectsPage {
                    objects: vec![("b2".to_owned(), 5), ("x y/ü.txt".to_owned(), 5)],
                    next_token: None,
                },
            ),
            (LIST_OBJECTS_EMPTY, ObjectsPage::default()),
        ];
        for (xml, want) in cases {
            assert_eq!(parse_list_objects_v2(xml).unwrap(), want);
        }
    }

    #[test]
    fn references_are_resolved_inside_keys() {
        let xml = "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>&lt;a&gt;&#x41;&#66;&quot;&apos;</Key><Size>1</Size></Contents></ListBucketResult>";
        assert_eq!(
            parse_list_objects_v2(xml).unwrap().objects,
            vec![("<a>AB\"'".to_owned(), 1)]
        );
    }

    #[test]
    fn error_fixtures() {
        let cases = [
            (
                ERROR_NO_SUCH_BUCKET,
                "NoSuchBucket",
                "The specified bucket does not exist.",
            ),
            (
                ERROR_NOT_EMPTY,
                "BucketNotEmpty",
                "The bucket you tried to delete is not empty.",
            ),
        ];
        for (xml, code, message) in cases {
            assert_eq!(
                parse_error(xml).unwrap(),
                S3ErrorBody {
                    code: code.to_owned(),
                    message: message.to_owned(),
                }
            );
        }
    }

    #[test]
    fn malformed_and_unexpected_replies_error() {
        assert!(matches!(
            parse_list_buckets("<ListAllMyBucketsResult><Buckets>"),
            Err(S3XmlError::Malformed(_))
        ));
        assert!(matches!(
            parse_list_buckets("<A></B>"),
            Err(S3XmlError::Malformed(_))
        ));
        assert!(matches!(
            parse_list_objects_v2(
                "<ListBucketResult><Contents><Key>&bogus;</Key></Contents></ListBucketResult>"
            ),
            Err(S3XmlError::Malformed(_))
        ));
        assert_eq!(
            parse_list_buckets(LIST_OBJECTS_EMPTY),
            Err(S3XmlError::UnexpectedRoot {
                expected: "ListAllMyBucketsResult",
                found: "ListBucketResult".to_owned(),
            })
        );
        assert_eq!(
            parse_error("not xml"),
            Err(S3XmlError::UnexpectedRoot {
                expected: "Error",
                found: String::new(),
            })
        );
        assert_eq!(
            parse_error("<Error><Message>x</Message></Error>"),
            Err(S3XmlError::MissingField("Code"))
        );
        assert_eq!(
            parse_list_objects_v2(
                "<ListBucketResult><Contents><Key>k</Key><Size>big</Size></Contents></ListBucketResult>"
            ),
            Err(S3XmlError::InvalidNumber {
                field: "Size",
                value: "big".to_owned(),
            })
        );
        assert_eq!(
            parse_list_objects_v2(
                "<ListBucketResult><IsTruncated>true</IsTruncated></ListBucketResult>"
            ),
            Err(S3XmlError::MissingField("NextContinuationToken"))
        );
        assert_eq!(
            parse_list_objects_v2(
                "<ListBucketResult><Contents><Size>1</Size></Contents></ListBucketResult>"
            ),
            Err(S3XmlError::MissingField("Key"))
        );
    }
}
