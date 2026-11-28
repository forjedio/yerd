use serde_json::{json, Value};

use super::*;

const T0: u64 = 1_700_000_000_000;
const SITE: &str = "blog";

fn sample(id: &str, traced: bool, duration_us: u64) -> ProxySample {
    ProxySample {
        id: id.to_owned(),
        site: SITE.to_owned(),
        php: "8.3".to_owned(),
        method: "GET".to_owned(),
        path: "/orders/1".to_owned(),
        script: None,
        status: 200,
        bytes: 100,
        content_type: Some("text/html".to_owned()),
        start_us: T0 * 1000,
        duration_us,
        aborted: false,
        proxy_error: false,
        traced,
    }
}

fn frame(id: &str, category: DumpCategory, seq: u32, t_us: u64, payload: Value) -> ExtFrame {
    ExtFrame {
        request_id: id.to_owned(),
        site: SITE.to_owned(),
        category,
        seq: Some(seq),
        t_us: Some(t_us),
        payload,
    }
}

fn query(id: &str, seq: u32, t_us: u64, binding: u64) -> ExtFrame {
    frame(
        id,
        DumpCategory::Query,
        seq,
        t_us,
        json!({
            "sql": "select * from `users` where `id` = ?",
            "bindings": [binding],
            "time_ms": 2.0,
            "row_count": 1,
            "file": "app/Http/Controllers/OrderController.php",
            "line": 42
        }),
    )
}

fn summary(id: &str, seq: u32, frames: u32, worker_requests: u64) -> ExtFrame {
    frame(
        id,
        DumpCategory::Request,
        seq,
        0,
        json!({
            "method": "GET", "uri": "/orders/1", "status": 200, "duration_ms": 10.0,
            "start_us": T0 * 1000 + 500, "duration_us": 9_000, "worker_pid": 1,
            "worker_requests": worker_requests,
            "route": {"uri": "orders/{order}", "name": "orders.show", "action": "OrderController@show"},
            "frames": frames, "frames_unsent": 0
        }),
    )
}

fn listed(a: &Aggregator, now: u64) -> Vec<FinalRequest> {
    a.recent_requests(
        SITE,
        InsightsRange::OneHour,
        None,
        InsightsSort::Recent,
        500,
        now,
    )
}

fn only(a: &Aggregator, now: u64) -> FinalRequest {
    let rows = listed(a, now);
    assert_eq!(rows.len(), 1, "{rows:?}");
    rows.into_iter().next().unwrap()
}

fn overview_1h(a: &Aggregator, now: u64) -> InsightsOverview {
    a.overview(SITE, InsightsRange::OneHour, now)
}

#[test]
fn sample_then_summary() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r1", true, 10_000), T0);
    assert_eq!(a.pending_len(), 1);
    a.on_frame(query("r1", 1, 100, 1), T0);
    a.on_frame(summary("r1", 2, 2, 3), T0);
    assert_eq!(a.pending_len(), 0);
    let r = only(&a, T0);
    assert_eq!(r.queries, 1);
    assert!(r.has_summary && r.has_detail && !r.cold);
    assert_eq!(r.dropped_frames, 0);
    assert_eq!(r.route_label, "/orders/{order}");
    assert_eq!(r.route_name.as_deref(), Some("orders.show"));
    assert_eq!(r.wait_us, Some(500));
    assert_eq!(r.db_us, Some(2_000));
    assert_eq!(r.app_us, Some(10_000 - 500 - 2_000));
    assert_eq!(r.ext_duration_us, Some(9_000));
    let o = overview_1h(&a, T0 + 1_000);
    assert_eq!(o.kpis.requests, 1);
    assert!(o.ext_seen);
}

#[test]
fn summary_then_sample() {
    let mut a = Aggregator::new();
    a.on_frame(query("r1", 1, 100, 1), T0);
    a.on_frame(summary("r1", 2, 2, 3), T0);
    assert_eq!(a.pending_len(), 1);
    assert!(listed(&a, T0).is_empty());
    a.on_sample(sample("r1", true, 10_000), T0 + 5);
    assert_eq!(a.pending_len(), 0);
    let r = only(&a, T0 + 5);
    assert_eq!((r.queries, r.dropped_frames), (1, 0));
}

#[test]
fn unsent_frames_are_not_waited_for_but_count_as_dropped() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r1", true, 10_000), T0);
    a.on_frame(query("r1", 1, 100, 1), T0);
    let mut s = summary("r1", 3, 3, 3);
    s.payload["frames_unsent"] = json!(1);
    a.on_frame(s, T0);
    assert_eq!(a.pending_len(), 0);
    let r = only(&a, T0);
    assert_eq!((r.queries, r.dropped_frames), (1, 1));
}

#[test]
fn untraced_sample_is_immediate_without_drops() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r1", false, 10_000), T0);
    assert_eq!(a.pending_len(), 0);
    let r = only(&a, T0);
    assert!(!r.has_summary && !r.traced);
    assert_eq!(r.dropped_frames, 0);
    assert_eq!(r.db_us, None);
    assert!(a.recent_detail(SITE, "r1").unwrap().events.is_empty());
}

#[test]
fn proxy_error_sample_is_immediate_without_drops() {
    let mut a = Aggregator::new();
    let mut s = sample("r1", false, 1_000);
    s.status = 500;
    s.proxy_error = true;
    a.on_sample(s, T0);
    let o = overview_1h(&a, T0 + 1_000);
    assert_eq!((o.kpis.requests, o.kpis.s5xx, o.dropped_frames), (1, 1, 0));
    assert_eq!(o.kpis.p50_ms, None);
}

#[test]
fn missing_summary_before_v2_is_not_dropped() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r1", true, 10_000), T0);
    a.tick(T0 + PROBE_GRACE_MS - 1);
    assert_eq!(a.pending_len(), 1);
    a.tick(T0 + PROBE_GRACE_MS);
    assert_eq!(a.pending_len(), 0);
    assert_eq!(only(&a, T0 + PROBE_GRACE_MS).dropped_frames, 0);
    assert_eq!(overview_1h(&a, T0 + PROBE_GRACE_MS).dropped_frames, 0);
}

#[test]
fn a_frame_before_v2_extends_the_wait_to_the_full_grace() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r1", true, 10_000), T0);
    a.on_frame(query("r1", 1, 1, 1), T0 + 10);
    a.tick(T0 + PROBE_GRACE_MS);
    assert_eq!(a.pending_len(), 1);
    a.tick(T0 + GRACE_MS);
    assert_eq!(a.pending_len(), 0);
}

#[test]
fn after_v2_a_traced_sample_waits_the_full_grace() {
    let mut a = Aggregator::new();
    a.on_sample(sample("seen", true, 10_000), T0);
    a.on_frame(summary("seen", 1, 1, 2), T0);
    a.on_sample(sample("r2", true, 10_000), T0);
    a.tick(T0 + PROBE_GRACE_MS);
    assert_eq!(a.pending_len(), 1);
    a.tick(T0 + GRACE_MS);
    assert_eq!(a.pending_len(), 0);
}

#[test]
fn missing_summary_after_v2_is_dropped_and_late_frames_not_recounted() {
    let mut a = Aggregator::new();
    a.on_sample(sample("seen", true, 10_000), T0);
    a.on_frame(summary("seen", 1, 1, 2), T0);
    a.on_sample(sample("lost", true, 10_000), T0);
    for seq in 1..=3 {
        a.on_frame(query("lost", seq, u64::from(seq), u64::from(seq)), T0);
    }
    a.tick(T0 + GRACE_MS);
    let lost = listed(&a, T0 + GRACE_MS)
        .into_iter()
        .find(|r| r.id == "lost")
        .unwrap();
    assert_eq!(lost.dropped_frames, 1);
    assert_eq!(overview_1h(&a, T0 + GRACE_MS).dropped_frames, 1);
    a.on_frame(summary("lost", 4, 4, 2), T0 + GRACE_MS + 1);
    a.tick(T0 + GRACE_MS + ORPHAN_MS + 1);
    assert_eq!(a.pending_len(), 0);
    assert_eq!(
        overview_1h(&a, T0 + GRACE_MS + ORPHAN_MS + 1).dropped_frames,
        1
    );
}

#[test]
fn slow_request_with_short_gaps_is_not_orphaned() {
    let mut a = Aggregator::new();
    let gap = ORPHAN_MS - 10_000;
    for (i, at) in [T0, T0 + gap, T0 + 2 * gap].into_iter().enumerate() {
        let seq = u32::try_from(i).unwrap() + 1;
        a.on_frame(query("slow", seq, u64::from(seq), u64::from(seq)), at);
        a.tick(at + gap - 1);
        assert_eq!(a.pending_len(), 1, "after frame {seq}");
    }
    let end = T0 + 2 * gap + 1_000;
    a.on_frame(summary("slow", 4, 4, 2), end);
    a.on_sample(sample("slow", true, 40_000_000), end);
    let r = only(&a, end);
    assert_eq!((r.queries, r.dropped_frames), (3, 0));
}

#[test]
fn true_orphan_is_counted_once() {
    let mut a = Aggregator::new();
    a.on_frame(query("o", 1, 1, 1), T0);
    a.tick(T0 + ORPHAN_MS - 1);
    assert_eq!(a.pending_len(), 1);
    a.tick(T0 + ORPHAN_MS);
    assert_eq!(a.pending_len(), 0);
    assert_eq!(overview_1h(&a, T0 + ORPHAN_MS).dropped_frames, 1);
    a.on_frame(query("o", 2, 2, 2), T0 + ORPHAN_MS + 1);
    a.tick(T0 + 3 * ORPHAN_MS);
    assert_eq!(overview_1h(&a, T0 + 3 * ORPHAN_MS).dropped_frames, 1);
}

#[test]
fn orphan_with_summary_counts_announced_frames_once() {
    let mut a = Aggregator::new();
    a.on_frame(summary("o", 5, 5, 2), T0);
    a.tick(T0 + ORPHAN_MS);
    assert_eq!(overview_1h(&a, T0 + ORPHAN_MS).dropped_frames, 5);
    a.on_frame(query("o", 1, 1, 1), T0 + ORPHAN_MS + 1);
    a.on_sample(sample("o", true, 1_000), T0 + ORPHAN_MS + 2);
    a.tick(T0 + 3 * ORPHAN_MS);
    assert_eq!(overview_1h(&a, T0 + 3 * ORPHAN_MS).dropped_frames, 5);
    assert!(listed(&a, T0 + 3 * ORPHAN_MS).is_empty());
}

#[test]
fn out_of_order_seq_sorts_by_time() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", true, 10_000), T0);
    a.on_frame(query("r", 3, 300, 3), T0);
    a.on_frame(query("r", 1, 100, 1), T0);
    a.on_frame(summary("r", 4, 4, 2), T0);
    assert_eq!(a.pending_len(), 1);
    a.on_frame(query("r", 2, 200, 2), T0);
    assert_eq!(a.pending_len(), 0);
    let d = a.recent_detail(SITE, "r").unwrap();
    let times: Vec<Option<u64>> = d.events.iter().map(|e| e.t_us).collect();
    assert_eq!(times, vec![Some(100), Some(200), Some(300)]);
    assert_eq!(
        d.events.iter().map(|e| e.idx).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(only(&a, T0).dropped_frames, 0);
}

#[test]
fn event_cap_counts_refused_frames_once() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", true, 10_000), T0);
    let n = u32::try_from(MAX_EVENTS_PER_REQUEST).unwrap() + 5;
    for seq in 1..=n {
        a.on_frame(query("r", seq, u64::from(seq), u64::from(seq)), T0);
    }
    a.on_frame(summary("r", n + 1, n + 1, 2), T0);
    assert_eq!(a.pending_len(), 0);
    let r = only(&a, T0);
    assert_eq!(r.dropped_frames, 5);
    assert_eq!(r.queries as usize, MAX_EVENTS_PER_REQUEST);
}

#[test]
fn byte_cap_refuses_frames() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", true, 10_000), T0);
    let big = "x".repeat(4 * 1024 * 1024);
    let n = u32::try_from(PENDING_BYTES_CAP / big.len()).unwrap() + 2;
    for seq in 1..=n {
        a.on_frame(
            frame(
                "r",
                DumpCategory::Dump,
                seq,
                u64::from(seq),
                json!({ "value_html": big }),
            ),
            T0,
        );
    }
    a.on_frame(summary("r", n + 1, n + 1, 2), T0);
    let r = only(&a, T0);
    assert!(r.dropped_frames >= 2, "{}", r.dropped_frames);
    assert_eq!(r.dumps + r.dropped_frames, n);
}

#[test]
fn detail_is_capped() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", true, 10_000), T0);
    let html = "h".repeat(100 * 1024);
    let sql = "s".repeat(20 * 1024);
    a.on_frame(
        frame(
            "r",
            DumpCategory::Query,
            1,
            1,
            json!({ "sql": sql, "time_ms": 1.0 }),
        ),
        T0,
    );
    for seq in 2..=21 {
        a.on_frame(
            frame(
                "r",
                DumpCategory::Dump,
                seq,
                u64::from(seq),
                json!({ "value_html": html, "note": sql }),
            ),
            T0,
        );
    }
    a.on_frame(summary("r", 22, 22, 2), T0);
    let d = a.recent_detail(SITE, "r").unwrap();
    assert!(d.truncated);
    assert!(d.events.len() < 21);
    let q = d.events[0].payload["sql"].as_str().unwrap();
    assert_eq!(q.len(), DETAIL_SQL_MAX);
    let dump = &d.events[1].payload;
    assert_eq!(dump["value_html"].as_str().unwrap().len(), DETAIL_HTML_MAX);
    assert_eq!(dump["note"].as_str().unwrap().len(), DETAIL_STRING_MAX);
    assert_eq!(only(&a, T0).dumps, 20);
}

#[test]
fn cold_request_is_listed_but_not_in_latency() {
    let mut a = Aggregator::new();
    a.on_sample(sample("cold", true, 900_000), T0);
    a.on_frame(summary("cold", 1, 1, 1), T0);
    a.on_sample(sample("warm", true, 10_000), T0);
    a.on_frame(summary("warm", 1, 1, 7), T0);
    let rows = listed(&a, T0);
    assert!(rows.iter().find(|r| r.id == "cold").unwrap().cold);
    let o = a.overview(SITE, InsightsRange::FiveMinutes, T0 + 1_000);
    assert_eq!((o.kpis.requests, o.cold), (2, 1));
    let p50 = o.kpis.p50_ms.unwrap();
    assert!(p50 < 20.0, "{p50}");
    let summary = rows.iter().find(|r| r.id == "cold").unwrap().to_summary();
    assert!(summary.cold);
}

#[test]
fn flush_by_time_bytes_and_force() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", false, 1_000), T0);
    assert!(a.take_flush(T0 + 1, false, None).is_empty());
    let batches = a.take_flush(T0 + FLUSH_INTERVAL_MS, false, None);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].ids(), vec!["r".to_owned()]);
    assert_eq!(batches[0].hours(), vec![hour_of(T0)]);
    assert!(a
        .take_flush(T0 + FLUSH_INTERVAL_MS + 1, true, None)
        .is_empty());

    a.on_sample(sample("f", false, 1_000), T0 + FLUSH_INTERVAL_MS + 2);
    assert_eq!(
        a.take_flush(T0 + FLUSH_INTERVAL_MS + 3, true, None).len(),
        1
    );
    a.on_sample(sample("g", false, 1_000), T0);
    assert!(a
        .take_flush(T0 + FLUSH_INTERVAL_MS + 4, true, Some("other"))
        .is_empty());
    assert_eq!(
        a.take_flush(T0 + FLUSH_INTERVAL_MS + 4, true, Some(SITE))
            .len(),
        1
    );

    let mut b = Aggregator::new();
    let html = "y".repeat(60 * 1024);
    let mut now = T0;
    for i in 0..10u32 {
        let id = format!("big{i}");
        b.on_sample(sample(&id, true, 1_000), now);
        for seq in 1..=17 {
            b.on_frame(
                frame(
                    &id,
                    DumpCategory::Dump,
                    seq,
                    u64::from(seq),
                    json!({ "value_html": html }),
                ),
                now,
            );
        }
        b.on_frame(summary(&id, 18, 18, 2), now);
        now += 1;
        if i < 6 {
            assert!(
                b.take_flush(now, false, None).is_empty(),
                "flushed early at {i}"
            );
        }
    }
    let batches = b.take_flush(now, false, None);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].ids().len(), 10);
}

#[test]
fn unflushed_entry_stays_listed_with_detail() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", true, 10_000), T0);
    a.on_frame(query("r", 1, 1, 1), T0);
    a.on_frame(summary("r", 2, 2, 2), T0);
    let later = T0 + 7 * 60_000;
    a.tick(later);
    assert_eq!(listed(&a, later).len(), 1);
    assert_eq!(a.recent_detail(SITE, "r").unwrap().events.len(), 1);
}

#[test]
fn confirmed_entry_drops_detail_and_trims() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", true, 10_000), T0);
    a.on_frame(query("r", 1, 1, 1), T0);
    a.on_frame(summary("r", 2, 2, 2), T0);
    let b = a.take_flush(T0 + 1, true, None).remove(0);
    assert!(b
        .per_hour
        .values()
        .any(|h| h.requests.iter().any(|(_, _, d)| d.is_some())));
    a.confirm_flush(SITE, b.generation, &b.ids());
    assert!(a.recent_detail(SITE, "r").is_none());
    a.tick(T0 + RECENT_MS);
    assert_eq!(listed(&a, T0 + RECENT_MS).len(), 1);
    a.tick(T0 + RECENT_MS + 1);
    assert!(listed(&a, T0 + RECENT_MS + 1).is_empty());
    assert_eq!(overview_1h(&a, T0 + RECENT_MS + 1).kpis.requests, 1);
}

#[test]
fn stale_generation_confirm_is_ignored() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", true, 10_000), T0);
    a.on_frame(query("r", 1, 1, 1), T0);
    a.on_frame(summary("r", 2, 2, 2), T0);
    let b = a.take_flush(T0 + 1, true, None).remove(0);
    a.confirm_flush(SITE, b.generation + 1, &b.ids());
    a.flush_failed(SITE, b.generation + 1, &b.ids(), &b.hours(), T0 + 1);
    assert!(a.recent_detail(SITE, "r").is_some());
    a.tick(T0 + RECENT_MS + 1);
    assert_eq!(listed(&a, T0 + RECENT_MS + 1).len(), 1);
    assert_eq!(overview_1h(&a, T0 + 2).dropped_requests, 0);
}

#[test]
fn failed_flush_counts_drops_trims_and_rewrites_dict() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", false, 1_000), T0);
    let b = a.take_flush(T0 + 1, true, None).remove(0);
    let hour = hour_of(T0);
    assert!(!b.per_hour[&hour].dict_entries.is_empty());
    a.on_sample(sample("s", false, 1_000), T0 + 2);
    let again = a.take_flush(T0 + 3, true, None).remove(0);
    assert!(again.per_hour[&hour].dict_entries.is_empty());
    a.flush_failed(SITE, b.generation, &b.ids(), &b.hours(), T0 + 4);
    assert_eq!(overview_1h(&a, T0 + 5).dropped_requests, 1);
    a.on_sample(sample("t", false, 1_000), T0 + 6);
    let third = a.take_flush(T0 + 7, true, None).remove(0);
    assert!(third.per_hour[&hour]
        .dict_entries
        .iter()
        .any(|(_, e)| matches!(e, DictEntry::Route { .. })));
    a.confirm_flush(SITE, again.generation, &again.ids());
    a.confirm_flush(SITE, third.generation, &third.ids());
    a.tick(T0 + RECENT_MS + 10);
    assert!(listed(&a, T0 + RECENT_MS + 10).is_empty());
}

#[test]
fn summary_settle_waits_for_in_flight_frames() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", true, 10_000), T0);
    a.on_frame(query("r", 1, 1, 1), T0);
    a.on_frame(summary("r", 3, 3, 2), T0);
    assert_eq!(a.pending_len(), 1);
    a.tick(T0 + SETTLE_MS - 1);
    assert_eq!(a.pending_len(), 1);
    a.on_frame(query("r", 2, 2, 2), T0 + 500);
    assert_eq!(a.pending_len(), 0);
    assert_eq!(only(&a, T0 + 500).dropped_frames, 0);

    let mut b = Aggregator::new();
    b.on_sample(sample("r", true, 10_000), T0);
    b.on_frame(query("r", 1, 1, 1), T0);
    b.on_frame(summary("r", 3, 3, 2), T0);
    b.tick(T0 + SETTLE_MS);
    assert_eq!(b.pending_len(), 0);
    assert_eq!(only(&b, T0 + SETTLE_MS).dropped_frames, 1);
}

#[test]
fn clear_resets_the_site_and_its_pending_joins() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", true, 10_000), T0);
    for seq in 1..=5 {
        a.on_frame(query("r", seq, u64::from(seq), u64::from(seq)), T0);
    }
    a.on_frame(summary("r", 6, 6, 2), T0);
    assert_eq!(a.open_issue_count(SITE), 1);
    let key = overview_1h(&a, T0 + 1).issues[0].key.clone();
    assert!(a.dismiss(SITE, crate::pure::model::parse_key_hex(&key).unwrap(), true));
    a.on_frame(query("p", 1, 1, 1), T0);
    a.on_sample(sample("q", true, 1_000), T0);
    assert_eq!(a.pending_len(), 2);
    a.clear(SITE, T0 + 2);
    assert_eq!(a.pending_len(), 0);
    assert_eq!(a.generation(SITE), 1);
    assert_eq!(a.open_issue_count(SITE), 0);
    assert!(a.dismissed_keys(SITE).is_empty());
    assert!(listed(&a, T0 + 2).is_empty());
    let o = overview_1h(&a, T0 + 2);
    assert_eq!(o.kpis.requests, 0);
    assert!(o.issues.is_empty());
    assert!(a.take_flush(T0 + 3, true, None).is_empty());
}

#[test]
fn load_site_requires_an_empty_site() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", false, 1_000), T0);
    let err = a.load_site(SITE, vec![], vec![], vec![], BTreeSet::new(), T0);
    assert!(matches!(err, Err(InsightsError::SiteNotEmpty { .. })));

    let mut src = Aggregator::new();
    src.on_sample(sample("r", true, 10_000), T0);
    for seq in 1..=5 {
        src.on_frame(query("r", seq, u64::from(seq), u64::from(seq)), T0);
    }
    src.on_frame(summary("r", 6, 6, 2), T0);
    let b = src.take_flush(T0 + 1, true, None).remove(0);
    let hour = hour_of(T0);
    let hb = &b.per_hour[&hour];
    let dict: Vec<(u64, u64, DictEntry)> = hb
        .dict_entries
        .iter()
        .map(|(k, e)| (hour, *k, e.clone()))
        .collect();
    let reqs: Vec<FinalRequest> = hb.requests.iter().map(|(_, r, _)| r.clone()).collect();

    let loaded: BTreeSet<u64> = dict.iter().map(|(_, k, _)| *k).collect();
    let mut dst = Aggregator::new();
    dst.clear(SITE, T0);
    dst.load_site(
        SITE,
        hb.rollup_deltas.clone(),
        dict,
        reqs,
        BTreeSet::new(),
        T0 + 2,
    )
    .unwrap();
    assert_eq!(dst.open_issue_count(SITE), 1);
    assert_eq!(dst.generation(SITE), 1);
    let o1 = overview_1h(&src, T0 + 2);
    let o2 = overview_1h(&dst, T0 + 2);
    assert_eq!(o1.kpis, o2.kpis);
    assert_eq!(o1.issues, o2.issues);
    assert_eq!(o1.routes, o2.routes);
    assert!(dst.recent_detail(SITE, "r").is_none());
    assert_eq!(listed(&dst, T0 + 2).len(), 1);
    dst.on_sample(sample("s", true, 1_000), T0 + 3);
    dst.on_frame(summary("s", 1, 1, 2), T0 + 3);
    let next = dst.take_flush(T0 + 4, true, None).remove(0);
    let rewritten: Vec<u64> = next.per_hour[&hour]
        .dict_entries
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| loaded.contains(k))
        .collect();
    assert!(rewritten.is_empty(), "{rewritten:?}");
    assert!(next.per_hour[&hour]
        .rollup_deltas
        .iter()
        .any(|d| d.routes.keys().any(|k| loaded.contains(k))));
}

#[test]
fn finish_request_uses_the_proxy_duration_as_base() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", true, 5_000), T0);
    let mut s = summary("r", 2, 2, 2);
    s.payload["duration_us"] = json!(50_000);
    a.on_frame(query("r", 1, 1, 1), T0);
    a.on_frame(s, T0);
    let r = only(&a, T0);
    assert_eq!(r.duration_us, 5_000);
    assert_eq!(r.ext_duration_us, Some(50_000));
    assert_eq!(r.app_us, Some(5_000 - 500 - 2_000));
}

#[test]
fn rollup_prune_retains_dict_and_recomputes_open_issues() {
    let mut a = Aggregator::new();
    a.on_sample(sample("r", true, 10_000), T0);
    for seq in 1..=5 {
        a.on_frame(query("r", seq, u64::from(seq), u64::from(seq)), T0);
    }
    a.on_frame(summary("r", 6, 6, 2), T0);
    let key = crate::pure::model::parse_key_hex(&overview_1h(&a, T0 + 1).issues[0].key).unwrap();
    a.dismiss(SITE, key, true);
    assert_eq!(a.open_issue_count(SITE), 0);
    assert_eq!(a.dismissed_keys(SITE), vec![key]);
    let b = a.take_flush(T0 + 1, true, None).remove(0);
    a.confirm_flush(SITE, b.generation, &b.ids());
    let later = minute_of(T0) + RETENTION_MS;
    a.on_sample(
        ProxySample {
            start_us: later * 1000,
            ..sample("fresh", false, 1_000)
        },
        later,
    );
    let out = a.tick(later);
    assert_eq!(out.dismissed_changed, vec![SITE.to_owned()]);
    assert!(a.dismissed_keys(SITE).is_empty());
    assert_eq!(a.open_issue_count(SITE), 0);
    let st = a.sites.get(SITE).unwrap();
    assert!(st.open_issues.is_empty());
    assert!(!st.dict.contains(key));
    assert_eq!(st.rollups.len(), 1);
    assert!(a.tick(later + 1).dismissed_changed.is_empty());
}

#[test]
fn open_issue_set_updates_on_fold() {
    let mut a = Aggregator::new();
    assert_eq!(a.open_issue_count(SITE), 0);
    a.on_sample(sample("r", true, 10_000), T0);
    for seq in 1..=5 {
        a.on_frame(query("r", seq, u64::from(seq), u64::from(seq)), T0);
    }
    a.on_frame(summary("r", 6, 6, 2), T0);
    assert_eq!(a.open_issue_count(SITE), 1);
    let d = a.recent_detail(SITE, "r").unwrap();
    assert_eq!(d.issues.len(), 1);
    assert_eq!(d.issues[0].event_indices, vec![0, 1, 2, 3, 4]);
}

#[test]
fn read_paths_never_create_site_state() {
    let a = Aggregator::new();
    assert_eq!(
        a.overview("nope", InsightsRange::OneDay, T0).kpis.requests,
        0
    );
    assert!(a
        .recent_requests(
            "nope",
            InsightsRange::OneDay,
            None,
            InsightsSort::Slowest,
            10,
            T0
        )
        .is_empty());
    assert!(a.recent_detail("nope", "x").is_none());
    assert_eq!(a.sites().count(), 0);
    let mut m = Aggregator::new();
    assert!(!m.dismiss("nope", 1, true));
    assert_eq!(m.sites().count(), 0);
}

#[test]
fn request_selection_filters_sorts_and_limits() {
    let mut a = Aggregator::new();
    for (i, dur) in [3_000u64, 1_000, 2_000].into_iter().enumerate() {
        let mut s = sample(&format!("r{i}"), false, dur);
        s.start_us += u64::try_from(i).unwrap() * 1_000_000;
        if i == 1 {
            s.path = "/checkout".into();
        }
        a.on_sample(s, T0 + 5_000);
    }
    let ids = |rows: Vec<FinalRequest>| rows.into_iter().map(|r| r.id).collect::<Vec<_>>();
    let now = T0 + 5_000;
    assert_eq!(
        ids(a.recent_requests(
            SITE,
            InsightsRange::FiveMinutes,
            None,
            InsightsSort::Recent,
            10,
            now
        )),
        vec!["r2", "r1", "r0"]
    );
    assert_eq!(
        ids(a.recent_requests(
            SITE,
            InsightsRange::FiveMinutes,
            None,
            InsightsSort::Slowest,
            2,
            now
        )),
        vec!["r0", "r2"]
    );
    assert_eq!(
        ids(a.recent_requests(
            SITE,
            InsightsRange::FiveMinutes,
            Some("checkout"),
            InsightsSort::Recent,
            10,
            now
        )),
        vec!["r1"]
    );
}
