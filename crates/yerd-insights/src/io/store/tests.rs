use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;
use yerd_ipc::DumpCategory;

use super::*;
use crate::pure::aggregator::Aggregator;
use crate::pure::contract::{ExtFrame, ProxySample};
use crate::pure::model::tests::request;
use crate::pure::model::DetailEvent;
use crate::pure::retention::HOUR_MS;

const T0: u64 = 1_700_000_000_000;
const SITE: &str = "blog";

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("insights"));
    (dir, store)
}

fn detail(id: &str) -> Arc<RequestDetail> {
    Arc::new(RequestDetail {
        id: id.to_owned(),
        events: vec![DetailEvent {
            idx: 0,
            category: DumpCategory::Query,
            t_us: Some(5),
            dur_us: Some(100),
            payload: json!({"sql": "select 1"}),
        }],
        issues: vec![],
        truncated: false,
    })
}

fn batch(reqs: &[(&str, u64, u64, bool)]) -> FlushBatch {
    let mut per_hour: BTreeMap<u64, HourBatch> = BTreeMap::new();
    for (id, ts, dur, with_detail) in reqs {
        let mut r = request(id, *ts, *dur);
        r.has_detail = *with_detail;
        per_hour.entry(hour_of(*ts)).or_default().requests.push((
            (*id).to_owned(),
            r,
            with_detail.then(|| detail(id)),
        ));
    }
    FlushBatch {
        site: SITE.to_owned(),
        generation: 0,
        per_hour,
    }
}

fn site_dir(s: &Store) -> PathBuf {
    s.root().join(SITE)
}

fn path(s: &Store, hour: u64, kind: FileKind) -> PathBuf {
    site_dir(s).join(file_name(hour, kind))
}

fn ids(rows: &[FinalRequest]) -> Vec<&str> {
    rows.iter().map(|r| r.id.as_str()).collect()
}

fn scan(s: &Store, sort: InsightsSort, limit: u32, now: u64) -> Vec<FinalRequest> {
    s.scan_requests(SITE, InsightsRange::OneDay, None, sort, limit, now)
        .unwrap()
}

fn sample(id: &str, ts_ms: u64) -> ProxySample {
    ProxySample {
        id: id.to_owned(),
        site: SITE.to_owned(),
        php: "8.3".to_owned(),
        method: "GET".to_owned(),
        path: "/orders/1".to_owned(),
        script: None,
        status: 200,
        bytes: 100,
        content_type: None,
        start_us: ts_ms * 1000,
        duration_us: 10_000,
        aborted: false,
        proxy_error: false,
        traced: true,
    }
}

fn frame(id: &str, category: DumpCategory, seq: u32, payload: serde_json::Value) -> ExtFrame {
    ExtFrame {
        request_id: id.to_owned(),
        site: SITE.to_owned(),
        category,
        seq: Some(seq),
        t_us: Some(10),
        payload,
    }
}

fn feed_n_plus_one(a: &mut Aggregator, id: &str, ts_ms: u64) {
    a.on_sample(sample(id, ts_ms), ts_ms);
    for i in 0..6u32 {
        a.on_frame(
            frame(
                id,
                DumpCategory::Query,
                i + 1,
                json!({
                    "sql": "select * from `users` where `id` = ?",
                    "bindings": [i],
                    "time_ms": 1.0,
                    "row_count": 1,
                    "file": "app/Http/Controllers/OrderController.php",
                    "line": 42
                }),
            ),
            ts_ms,
        );
    }
    a.on_frame(
        frame(
            id,
            DumpCategory::Request,
            7,
            json!({
                "method": "GET", "uri": "/orders/1", "status": 200, "duration_ms": 9.0,
                "start_us": ts_ms * 1000, "duration_us": 9_000, "worker_pid": 1,
                "worker_requests": 3,
                "route": {"uri": "orders/{order}", "name": "orders.show", "action": null},
                "frames": 7, "frames_unsent": 0
            }),
        ),
        ts_ms,
    );
}

#[test]
fn append_then_reload_matches_the_fold() {
    let (_d, s) = store();
    let mut a = Aggregator::new();
    feed_n_plus_one(&mut a, "r1", T0);
    feed_n_plus_one(&mut a, "r2", T0 + 1_000);
    let now = T0 + 2_000;
    let batches = a.take_flush(now, true, None);
    assert_eq!(batches.len(), 1);
    let written = s.append_batch(&batches[0]).unwrap();
    assert!(written > 0);
    assert_eq!(s.generation(SITE), 1);
    a.confirm_flush(SITE, batches[0].generation, &batches[0].ids());

    let loaded = s.load_all(now).unwrap();
    assert_eq!(loaded.len(), 1);
    let l = loaded.into_iter().next().unwrap();
    assert!(l.error.is_none());
    assert!(!l.dict_entries.is_empty());
    assert_eq!(l.recent.len(), 2);
    let mut b = Aggregator::new();
    b.load_site(SITE, l.rollups, l.dict_entries, l.recent, l.dismissed, now)
        .unwrap();
    for range in [InsightsRange::OneHour, InsightsRange::OneDay] {
        let want = a.overview(SITE, range, now);
        let got = b.overview(SITE, range, now);
        assert_eq!(got, want);
        assert_eq!(got.routes[0].label, "/orders/{order}");
        assert!(got.queries[0].fingerprint.contains("users"));
        assert!(!got.issues.is_empty());
    }
    assert_eq!(b.open_issue_count(SITE), a.open_issue_count(SITE));
}

#[test]
fn torn_tail_is_truncated_before_the_next_append() {
    let (_d, s) = store();
    s.append_batch(&batch(&[("a", T0, 1, false)])).unwrap();
    let hour = hour_of(T0);
    for kind in [FileKind::Req, FileKind::Roll] {
        let p = path(&s, hour, kind);
        let mut f = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&p)
            .unwrap();
        f.write_all(b"{\"r\":{\"id\":\"torn").unwrap();
    }
    let mut b = batch(&[("b", T0 + 1, 1, false)]);
    let mut delta = MinuteRollup::new(T0);
    delta.static_skipped = 4;
    b.per_hour
        .get_mut(&hour)
        .unwrap()
        .rollup_deltas
        .push(delta.clone());
    s.append_batch(&b).unwrap();

    let req = fs::read(path(&s, hour, FileKind::Req)).unwrap();
    let decoded: Vec<String> = lines(&req)
        .map(|l| decode_req_line(l).unwrap().0.id)
        .collect();
    assert_eq!(decoded, vec!["a", "b"]);
    let roll = fs::read(path(&s, hour, FileKind::Roll)).unwrap();
    let rolled: Vec<RollLine> = lines(&roll).map(|l| decode_roll_line(l).unwrap()).collect();
    assert_eq!(rolled, vec![RollLine::Delta(delta)]);
}

#[test]
fn newline_termination_edge_cases() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f");
    ensure_newline_terminated(&p).unwrap();
    assert!(!p.exists());
    let big_torn = "x".repeat(usize::try_from(TAIL_CHUNK).unwrap() * 2 + 5);
    let cases: &[(&str, &str)] = &[
        ("", ""),
        ("a\n", "a\n"),
        ("a\nb", "a\n"),
        ("torn", ""),
        ("a\nb\nc", "a\nb\n"),
    ];
    for (input, want) in cases {
        fs::write(&p, input).unwrap();
        ensure_newline_terminated(&p).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), *want, "{input:?}");
    }
    fs::write(&p, format!("keep\n{big_torn}")).unwrap();
    ensure_newline_terminated(&p).unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), "keep\n");
}

#[test]
fn det_double_append_keeps_earlier_locators_valid() {
    let (_d, s) = store();
    let hour = hour_of(T0);
    s.append_batch(&batch(&[("a", T0, 1, true)])).unwrap();
    let mut f = OpenOptions::new()
        .append(true)
        .open(path(&s, hour, FileKind::Det))
        .unwrap();
    f.write_all(&[0x1f, 0x8b, 0x08, 0x00, 0xde, 0xad]).unwrap();
    s.append_batch(&batch(&[("b", T0 + 5, 1, true)])).unwrap();
    for (id, ts) in [("a", T0), ("b", T0 + 5)] {
        let (r, d) = s.find_detail(SITE, id, ts).unwrap().unwrap();
        assert_eq!(r.id, id);
        assert_eq!(d.id, id);
        assert_eq!(d.events.len(), 1);
    }
}

#[test]
fn torn_lines_are_skipped_on_load() {
    let (_d, s) = store();
    let now = T0 + 1_000;
    s.append_batch(&batch(&[("a", T0, 1, false)])).unwrap();
    let mut f = OpenOptions::new()
        .append(true)
        .open(path(&s, hour_of(T0), FileKind::Req))
        .unwrap();
    f.write_all(b"garbage\n{\"r\":").unwrap();
    let loaded = s.load_all(now).unwrap();
    assert_eq!(ids(&loaded[0].recent), vec!["a"]);
}

#[test]
fn load_with_missing_root_is_empty() {
    let (_d, s) = store();
    assert!(s.load_all(T0).unwrap().is_empty());
    assert!(s.prune(T0).unwrap().is_empty());
}

#[test]
fn load_with_one_unreadable_site_keeps_the_others() {
    let (_d, s) = store();
    s.append_batch(&batch(&[("a", T0, 1, false)])).unwrap();
    let mut other = batch(&[("b", T0, 1, false)]);
    other.site = "shop".to_owned();
    s.append_batch(&other).unwrap();
    fs::create_dir_all(
        s.root()
            .join("shop")
            .join(file_name(hour_of(T0) - 1, FileKind::Roll)),
    )
    .unwrap();
    let loaded = s.load_all(T0 + 1).unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].site, SITE);
    assert!(loaded[0].error.is_none());
    assert_eq!(ids(&loaded[0].recent), vec!["a"]);
    assert_eq!(loaded[1].site, "shop");
    assert!(loaded[1].error.is_some());
    assert!(loaded[1].recent.is_empty() && loaded[1].rollups.is_empty());
}

#[test]
fn load_reads_dismissed_and_only_recent_requests() {
    let (_d, s) = store();
    let now = T0 + 10 * 60_000;
    s.append_batch(&batch(&[
        ("old", T0, 1, false),
        ("new", now - 1_000, 1, false),
    ]))
    .unwrap();
    s.save_dismissed(SITE, &[1, u64::MAX]).unwrap();
    let l = s.load_all(now).unwrap().remove(0);
    assert_eq!(ids(&l.recent), vec!["new"]);
    assert_eq!(l.dismissed, BTreeSet::from([1, u64::MAX]));
    assert_eq!(s.load_dismissed(SITE).unwrap(), l.dismissed);
    s.save_dismissed(SITE, &[]).unwrap();
    assert!(s.load_dismissed(SITE).unwrap().is_empty());
    assert!(!site_dir(&s).join(DISMISSED_FILE).exists());
}

#[test]
fn prune_by_age_returns_removed_pairs() {
    let (_d, s) = store();
    let old = T0;
    let fresh = T0 + 24 * HOUR_MS;
    s.append_batch(&batch(&[("a", old, 1, true), ("b", fresh, 1, true)]))
        .unwrap();
    let g = s.generation(SITE);
    let now = fresh + HOUR_MS;
    assert_eq!(s.prune(now).unwrap(), vec![(SITE.to_owned(), hour_of(old))]);
    assert!(s.generation(SITE) > g);
    for kind in [FileKind::Roll, FileKind::Req, FileKind::Det] {
        assert!(!path(&s, hour_of(old), kind).exists());
    }
    assert!(path(&s, hour_of(fresh), FileKind::Req).exists());
    assert!(s.prune(now).unwrap().is_empty());
}

#[test]
fn prune_enforces_the_cap_within_one_hour_span() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("insights");
    let unbounded = Store::new(root.clone());
    let now = T0 + 3 * HOUR_MS;
    for h in 0..3u64 {
        let ts = T0 + h * HOUR_MS;
        let owned: Vec<(String, u64)> = (0..10).map(|i| (format!("h{h}r{i}"), ts + i)).collect();
        let rows: Vec<(&str, u64, u64, bool)> = owned
            .iter()
            .map(|(id, t)| (id.as_str(), *t, 1, true))
            .collect();
        unbounded.append_batch(&batch(&rows)).unwrap();
    }
    assert!(unbounded.prune(now).unwrap().is_empty());
    let total = walk_len(&root);
    let oldest: u64 = [FileKind::Roll, FileKind::Req, FileKind::Det]
        .into_iter()
        .map(|k| fs::metadata(path(&unbounded, hour_of(T0), k)).map_or(0, |m| m.len()))
        .sum();
    let capped = Store::with_cap(root.clone(), total - oldest);
    let removed = capped.prune(now).unwrap();
    assert_eq!(removed, vec![(SITE.to_owned(), hour_of(T0))]);
    assert_eq!(walk_len(&root), total - oldest);
    let tighter = Store::with_cap(root.clone(), 1);
    assert_eq!(tighter.prune(now).unwrap().len(), 2);
    assert_eq!(walk_len(&root), 0);
}

fn walk_len(p: &Path) -> u64 {
    fs::read_dir(p)
        .unwrap()
        .flatten()
        .map(|e| {
            let m = e.metadata().unwrap();
            if m.is_dir() {
                walk_len(&e.path())
            } else {
                m.len()
            }
        })
        .sum()
}

#[test]
fn pruning_an_hour_removes_its_dictionary_lines() {
    let (_d, s) = store();
    let mut b = batch(&[]);
    let mut hb = HourBatch::default();
    hb.dict_entries.push((
        7,
        DictEntry::Sql {
            fingerprint: "select ?".into(),
            sample_sql: "select 1".into(),
        },
    ));
    b.per_hour.insert(hour_of(T0), hb);
    s.append_batch(&b).unwrap();
    assert_eq!(s.load_all(T0 + 1).unwrap()[0].dict_entries.len(), 1);
    s.prune(T0 + 26 * HOUR_MS).unwrap();
    let after = s.load_all(T0 + 1).unwrap();
    assert!(after.iter().all(|l| l.dict_entries.is_empty()));
}

#[test]
fn find_detail_reads_only_the_target_hour() {
    let (_d, s) = store();
    s.append_batch(&batch(&[("a", T0, 1, true)])).unwrap();
    assert!(s.find_detail(SITE, "a", T0).unwrap().is_some());
    assert!(s.find_detail(SITE, "a", T0 + HOUR_MS).unwrap().is_none());
    assert!(s.find_detail(SITE, "nope", T0).unwrap().is_none());
    assert!(s.find_detail("other", "a", T0).unwrap().is_none());
}

#[test]
fn find_detail_without_detail_is_empty() {
    let (_d, s) = store();
    s.append_batch(&batch(&[("a", T0, 1, false)])).unwrap();
    let (r, d) = s.find_detail(SITE, "a", T0).unwrap().unwrap();
    assert_eq!(r.id, "a");
    assert!(d.events.is_empty());
}

#[test]
fn find_detail_rejects_a_locator_for_another_id() {
    let (_d, s) = store();
    s.append_batch(&batch(&[("a", T0, 1, true)])).unwrap();
    let hour = hour_of(T0);
    let a_line = fs::read_to_string(path(&s, hour, FileKind::Req)).unwrap();
    let (_, loc) = decode_req_line(a_line.trim_end()).unwrap();
    let forged = encode_req_line(&request("z", T0, 1), loc).unwrap();
    append_bytes(
        &path(&s, hour, FileKind::Req),
        format!("{forged}\n").as_bytes(),
    )
    .unwrap();
    assert!(s.find_detail(SITE, "z", T0).unwrap().is_none());
}

#[test]
fn scan_order_and_limit() {
    let (_d, s) = store();
    let now = T0 + 2 * HOUR_MS;
    s.append_batch(&batch(&[
        ("a", T0, 50, false),
        ("b", T0 + 10, 10, false),
        ("c", T0 + HOUR_MS, 30, false),
        ("d", T0 + HOUR_MS + 5, 40, false),
        ("e", now - 1, 20, false),
    ]))
    .unwrap();
    assert_eq!(
        ids(&scan(&s, InsightsSort::Recent, 3, now)),
        vec!["e", "d", "c"]
    );
    assert_eq!(
        ids(&scan(&s, InsightsSort::Recent, 10, now)),
        vec!["e", "d", "c", "b", "a"]
    );
    assert_eq!(
        ids(&scan(&s, InsightsSort::Slowest, 2, now)),
        vec!["a", "d"]
    );
    assert!(scan(&s, InsightsSort::Recent, 0, now).is_empty());
    let hour_only = s
        .scan_requests(
            SITE,
            InsightsRange::OneHour,
            None,
            InsightsSort::Recent,
            10,
            now,
        )
        .unwrap();
    assert_eq!(ids(&hour_only), vec!["e", "d", "c"]);
    let filtered = s
        .scan_requests(
            SITE,
            InsightsRange::OneDay,
            Some("post"),
            InsightsSort::Recent,
            10,
            now,
        )
        .unwrap();
    assert!(filtered.is_empty());
    assert!(s
        .scan_requests(
            "other",
            InsightsRange::OneDay,
            None,
            InsightsSort::Recent,
            10,
            now
        )
        .unwrap()
        .is_empty());
}

#[test]
fn scan_cache_reuse_rules() {
    let (_d, s) = store();
    let now = T0 + 1_000;
    s.append_batch(&batch(&[("a", T0, 1, false)])).unwrap();
    assert_eq!(ids(&scan(&s, InsightsSort::Recent, 10, now)), vec!["a"]);

    s.append_batch(&batch(&[("b", T0 + 1, 1, false)])).unwrap();
    assert_eq!(
        ids(&scan(&s, InsightsSort::Recent, 10, now + 1_000)),
        vec!["a"],
        "reused within 30 s after a generation bump"
    );
    assert_eq!(
        ids(&scan(&s, InsightsSort::Recent, 10, now + SCAN_REUSE_MS)),
        vec!["b", "a"],
        "rescanned after 30 s"
    );

    let later = now + SCAN_REUSE_MS + 1;
    assert_eq!(
        ids(&scan(
            &s,
            InsightsSort::Recent,
            10,
            later + 10 * SCAN_REUSE_MS
        )),
        vec!["b", "a"],
        "reused while the generation is unchanged, however old"
    );

    s.append_batch(&batch(&[("c", T0 + 2, 1, false)])).unwrap();
    assert_eq!(
        ids(&scan(
            &s,
            InsightsSort::Recent,
            1,
            later + 10 * SCAN_REUSE_MS + 1
        )),
        vec!["c"],
        "a new key replaces the entry"
    );
    assert_eq!(lock(&s.cache).len(), 1);

    s.clear_site(SITE).unwrap();
    assert!(lock(&s.cache).is_empty());
    assert!(scan(&s, InsightsSort::Recent, 1, later).is_empty());
}

#[test]
fn an_idle_cache_entry_is_rescanned_once_a_row_ages_out() {
    let (_d, s) = store();
    let one_hour = |now| {
        s.scan_requests(
            SITE,
            InsightsRange::OneHour,
            None,
            InsightsSort::Recent,
            10,
            now,
        )
        .unwrap()
    };
    s.append_batch(&batch(&[
        ("old", T0, 1, false),
        ("new", T0 + 30 * 60_000, 1, false),
    ]))
    .unwrap();
    let now = T0 + 31 * 60_000;
    assert_eq!(ids(&one_hour(now)), vec!["new", "old"]);
    assert_eq!(
        ids(&one_hour(now + 40 * 60_000)),
        vec!["new"],
        "a row older than the range forces a rescan without a generation bump"
    );
}

#[test]
fn a_scan_overlapping_a_clear_is_not_stored() {
    let (_d, s) = store();
    let now = T0 + 1_000;
    s.append_batch(&batch(&[("a", T0, 1, false)])).unwrap();
    let key: ScanKey = (InsightsRange::OneDay, None, InsightsSort::Recent, 10);
    let generation = s.generation(SITE);
    let epoch = s.clear_epoch(SITE);
    let results = s.read_requests(SITE, &key, now).unwrap();
    s.clear_site(SITE).unwrap();
    s.store_scan(SITE, key, generation, epoch, now, results);
    assert!(lock(&s.cache).is_empty());
    assert!(scan(&s, InsightsSort::Recent, 10, now + 1).is_empty());
}

#[test]
fn an_entry_from_before_a_clear_is_never_reused() {
    let (_d, s) = store();
    let now = T0 + 1_000;
    s.append_batch(&batch(&[("a", T0, 1, false)])).unwrap();
    assert_eq!(scan(&s, InsightsSort::Recent, 10, now).len(), 1);
    let key: ScanKey = (InsightsRange::OneDay, None, InsightsSort::Recent, 10);
    let stale = vec![request("a", T0, 1)];
    let epoch_before = s.clear_epoch(SITE);
    s.clear_site(SITE).unwrap();
    lock(&s.cache).insert(
        SITE.to_owned(),
        CachedScan {
            epoch: epoch_before,
            generation: s.generation(SITE),
            scanned_at_ms: now,
            key,
            results: stale,
        },
    );
    assert!(scan(&s, InsightsSort::Recent, 10, now + 1).is_empty());
}

#[test]
fn a_generation_bump_during_a_scan_expires_with_the_window() {
    let (_d, s) = store();
    let now = T0 + 1_000;
    s.append_batch(&batch(&[("a", T0, 1, false)])).unwrap();
    let key: ScanKey = (InsightsRange::OneDay, None, InsightsSort::Recent, 10);
    let generation = s.generation(SITE);
    let epoch = s.clear_epoch(SITE);
    let results = s.read_requests(SITE, &key, now).unwrap();
    s.append_batch(&batch(&[("b", T0 + 1, 1, false)])).unwrap();
    s.store_scan(SITE, key, generation, epoch, now, results);
    assert_eq!(ids(&scan(&s, InsightsSort::Recent, 10, now + 1)), vec!["a"]);
    assert_eq!(
        ids(&scan(&s, InsightsSort::Recent, 10, now + SCAN_REUSE_MS)),
        vec!["b", "a"]
    );
}

#[test]
fn clear_removes_everything_for_the_site() {
    let (_d, s) = store();
    s.append_batch(&batch(&[("a", T0, 1, true)])).unwrap();
    s.save_dismissed(SITE, &[3]).unwrap();
    let g = s.generation(SITE);
    s.clear_site(SITE).unwrap();
    assert!(!site_dir(&s).exists());
    assert!(s.generation(SITE) > g);
    assert!(s.load_all(T0 + 1).unwrap().is_empty());
    assert!(s.find_detail(SITE, "a", T0).unwrap().is_none());
    s.clear_site(SITE).unwrap();
}

#[test]
fn append_to_an_unwritable_root_fails_but_bumps_the_generation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("insights");
    fs::write(&root, b"not a dir").unwrap();
    let s = Store::new(root);
    assert!(s.append_batch(&batch(&[("a", T0, 1, true)])).is_err());
    assert_eq!(s.generation(SITE), 1);
    assert!(s.load_all(T0).is_err());
}

#[test]
fn site_names_round_trip_through_dirs() {
    let (_d, s) = store();
    let mut b = batch(&[("a", T0, 1, false)]);
    b.site = "My Site".to_owned();
    s.append_batch(&b).unwrap();
    let loaded = s.load_all(T0 + 1).unwrap();
    assert_eq!(loaded[0].site, "My Site");
}
