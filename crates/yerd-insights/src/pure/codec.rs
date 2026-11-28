//! On-disk encodings.
//!
//! * `.roll`: NDJSON, each line either a rollup delta `{"m":minute,...}` or a
//!   dictionary entry `{"d":key,"e":entry}`. Dictionary lines live in the
//!   hour's `.roll`, so they age out with it.
//! * `.req`: NDJSON, one [`FinalRequest`] per line with an optional `loc` into
//!   the same hour's `.det`. The hour is always `hour_of(ts_ms)`.
//! * `.det`: one gzip member per request (`Compression::fast()`).
//!
//! Encoders return the line without its trailing newline. Decoders return
//! `Err` on torn or garbage input; callers skip such lines.

use std::io::{Read, Write};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::InsightsError;
use crate::pure::dict::DictEntry;
use crate::pure::model::{FinalRequest, RequestDetail};
use crate::pure::rollup::MinuteRollup;

/// Where a request's detail member sits in its hour's `.det` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailLoc {
    /// Byte offset of the gzip member.
    pub offset: u64,
    /// Member length in bytes.
    pub len: u64,
}

/// One decoded `.roll` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollLine {
    /// A rollup delta, added to the minute's running total on load.
    Delta(MinuteRollup),
    /// A dictionary entry.
    Dict(u64, DictEntry),
}

#[derive(Serialize, Deserialize)]
struct DictLine {
    d: u64,
    e: DictEntry,
}

#[derive(Serialize, Deserialize)]
struct ReqLine {
    r: FinalRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    loc: Option<DetailLoc>,
}

fn enc(e: &serde_json::Error) -> InsightsError {
    InsightsError::Encode(e.to_string())
}

fn dec(e: &impl std::fmt::Display) -> InsightsError {
    InsightsError::Decode(e.to_string())
}

/// Encode a rollup delta line.
pub fn encode_roll_delta(delta: &MinuteRollup) -> Result<String, InsightsError> {
    serde_json::to_string(delta).map_err(|e| enc(&e))
}

/// Encode a dictionary line.
pub fn encode_roll_dict(key: u64, entry: &DictEntry) -> Result<String, InsightsError> {
    serde_json::to_string(&DictLine {
        d: key,
        e: entry.clone(),
    })
    .map_err(|e| enc(&e))
}

/// Decode one `.roll` line.
pub fn decode_roll_line(line: &str) -> Result<RollLine, InsightsError> {
    let v: Value = serde_json::from_str(line).map_err(|e| dec(&e))?;
    if v.get("d").is_some() {
        let l: DictLine = serde_json::from_value(v).map_err(|e| dec(&e))?;
        Ok(RollLine::Dict(l.d, l.e))
    } else if v.get("m").is_some() {
        serde_json::from_value(v)
            .map(RollLine::Delta)
            .map_err(|e| dec(&e))
    } else {
        Err(InsightsError::Decode("unrecognised roll line".to_owned()))
    }
}

/// Encode one `.req` line.
pub fn encode_req_line(
    req: &FinalRequest,
    loc: Option<DetailLoc>,
) -> Result<String, InsightsError> {
    serde_json::to_string(&ReqLine {
        r: req.clone(),
        loc,
    })
    .map_err(|e| enc(&e))
}

/// Decode one `.req` line.
pub fn decode_req_line(line: &str) -> Result<(FinalRequest, Option<DetailLoc>), InsightsError> {
    let l: ReqLine = serde_json::from_str(line).map_err(|e| dec(&e))?;
    Ok((l.r, l.loc))
}

/// Encode a detail as one gzip member.
pub fn encode_detail(detail: &RequestDetail) -> Result<Vec<u8>, InsightsError> {
    let json = serde_json::to_vec(detail).map_err(|e| enc(&e))?;
    let mut gz = GzEncoder::new(Vec::with_capacity(json.len() / 3 + 64), Compression::fast());
    gz.write_all(&json)
        .map_err(|e| InsightsError::Encode(e.to_string()))?;
    gz.finish()
        .map_err(|e| InsightsError::Encode(e.to_string()))
}

/// Decode one gzip member produced by [`encode_detail`].
pub fn decode_detail(bytes: &[u8]) -> Result<RequestDetail, InsightsError> {
    let mut json = Vec::new();
    GzDecoder::new(bytes)
        .read_to_end(&mut json)
        .map_err(|e| dec(&e))?;
    serde_json::from_slice(&json).map_err(|e| dec(&e))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use serde_json::json;
    use yerd_ipc::DumpCategory;

    use super::*;
    use crate::pure::issues::IssueKind;
    use crate::pure::model::tests::request;
    use crate::pure::model::DetailEvent;
    use crate::pure::route::RouteKind;

    fn delta() -> MinuteRollup {
        let mut m = MinuteRollup::new(1_700_000_000_000);
        let mut r = request("a", 1_700_000_000_000, 12_000);
        r.issues = vec![crate::pure::model::RequestIssue {
            kind: IssueKind::Duplicate,
            key: u64::MAX,
            count: 2,
            time_us: 5,
            fp_hash: Some(u64::MAX - 1),
            file: None,
            line: None,
        }];
        m.fold_request(&r);
        m.static_skipped = 3;
        m
    }

    #[test]
    fn roll_lines_round_trip() {
        let d = delta();
        let line = encode_roll_delta(&d).unwrap();
        assert!(line.starts_with("{\"m\":"), "{line}");
        assert!(!line.contains('\n'));
        assert_eq!(decode_roll_line(&line).unwrap(), RollLine::Delta(d));
        let entries = [
            DictEntry::Route {
                method: "GET".into(),
                label: "/a".into(),
                kind: RouteKind::Template,
                name: Some("a".into()),
                action: None,
            },
            DictEntry::Sql {
                fingerprint: "select ?".into(),
                sample_sql: "select 1".into(),
            },
            DictEntry::Issue {
                kind: IssueKind::NPlusOne,
                fp_hash: Some(u64::MAX),
                file: Some("f.php".into()),
                line: Some(2),
                route_id: 9,
            },
        ];
        for e in entries {
            let line = encode_roll_dict(u64::MAX, &e).unwrap();
            assert_eq!(
                decode_roll_line(&line).unwrap(),
                RollLine::Dict(u64::MAX, e)
            );
        }
    }

    #[test]
    fn req_lines_round_trip() {
        let r = request("abc", 5, 1);
        let loc = Some(DetailLoc {
            offset: 10,
            len: 20,
        });
        for l in [None, loc] {
            let line = encode_req_line(&r, l).unwrap();
            assert_eq!(decode_req_line(&line).unwrap(), (r.clone(), l));
        }
    }

    #[test]
    fn torn_and_garbage_lines_are_rejected() {
        let line = encode_roll_delta(&delta()).unwrap();
        let torn = &line[..line.len() / 2];
        assert!(decode_roll_line(torn).is_err());
        assert!(decode_roll_line("{\"x\":1}").is_err());
        assert!(decode_roll_line("garbage").is_err());
        let req = encode_req_line(&request("a", 1, 1), None).unwrap();
        assert!(decode_req_line(&req[..req.len() - 1]).is_err());
        assert!(decode_req_line("").is_err());
    }

    #[test]
    fn gzip_member_round_trip() {
        let d = RequestDetail {
            id: "abc".into(),
            events: vec![DetailEvent {
                idx: 0,
                category: DumpCategory::Query,
                t_us: Some(5),
                dur_us: Some(100),
                payload: json!({"sql": "select 1", "bindings": []}),
            }],
            issues: vec![],
            truncated: false,
        };
        let bytes = encode_detail(&d).unwrap();
        assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
        assert_eq!(decode_detail(&bytes).unwrap(), d);
        let mut two = bytes.clone();
        two.extend(encode_detail(&RequestDetail::empty("x")).unwrap());
        assert_eq!(decode_detail(&two[bytes.len()..]).unwrap().id, "x");
        assert!(decode_detail(&bytes[..bytes.len() / 2]).is_err());
        assert!(decode_detail(b"not gzip").is_err());
    }
}
