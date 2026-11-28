use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use yerd_core::{PhpVersion, Site};
use yerd_insights::pure::aggregator::RECENT_MS;
use yerd_insights::pure::retention::{file_name, hour_of, FileKind};
use yerd_insights::ExtFrame;
use yerd_ipc::{DumpCategory, InsightsRange, InsightsSort, Request, Response};
use yerd_proxy::insights::{RequestClock, RequestInfo};

use super::*;
use crate::test_support::state_in;

const SITE: &str = "blog";

fn sample(id: &str, now: u64, traced: bool) -> yerd_insights::ProxySample {
    yerd_insights::ProxySample {
        id: id.to_owned(),
        site: SITE.to_owned(),
        php: "8.3".to_owned(),
        method: "GET".to_owned(),
        path: "/orders/1".to_owned(),
        script: None,
        status: 200,
        bytes: 10,
        content_type: None,
        start_us: now * 1000,
        duration_us: 10_000,
        aborted: false,
        proxy_error: false,
        traced,
    }
}

fn query_frame(id: &str, seq: u32, binding: u32) -> ExtFrame {
    ExtFrame {
        request_id: id.to_owned(),
        site: SITE.to_owned(),
        category: DumpCategory::Query,
        seq: Some(seq),
        t_us: Some(10),
        payload: json!({
            "sql": "select * from `users` where `id` = ?",
            "bindings": [binding],
            "time_ms": 1.0,
            "row_count": 1,
            "file": "app/Http/Controllers/OrderController.php",
            "line": 42
        }),
    }
}

fn summary_frame(id: &str, seq: u32, frames: u32, now: u64) -> ExtFrame {
    ExtFrame {
        request_id: id.to_owned(),
        site: SITE.to_owned(),
        category: DumpCategory::Request,
        seq: Some(seq),
        t_us: Some(0),
        payload: json!({
            "method": "GET", "uri": "/orders/1", "status": 200, "duration_ms": 9.0,
            "start_us": now * 1000, "duration_us": 9_000, "worker_pid": 1,
            "worker_requests": 2,
            "route": {"uri": "orders/{order}", "name": "orders.show", "action": null},
            "frames": frames, "frames_unsent": 0
        }),
    }
}

fn feed_n_plus_one(agg: &mut Aggregator, id: &str, now: u64) {
    agg.on_sample(sample(id, now, true), now);
    for i in 0..6u32 {
        agg.on_frame(query_frame(id, i + 1, i), now);
    }
    agg.on_frame(summary_frame(id, 7, 7, now), now);
}

fn insights_at(root: &Path) -> Insights {
    Insights::new(
        Store::new(root.to_path_buf()),
        7,
        &yerd_config::Config::default(),
    )
}

fn site_dir(state: &DaemonState) -> std::path::PathBuf {
    state.insights.store.root().join(SITE)
}

fn listed(agg: &Aggregator, now: u64) -> Vec<yerd_insights::FinalRequest> {
    agg.recent_requests(
        SITE,
        InsightsRange::OneDay,
        None,
        InsightsSort::Recent,
        500,
        now,
    )
}

async fn wait_for<F: Fn() -> bool>(cond: F) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met in time");
}

fn info() -> RequestInfo {
    RequestInfo {
        php: "8.3".to_owned(),
        method: "GET".to_owned(),
        path: "/orders/1".to_owned(),
        script: None,
    }
}

fn record_through_tap(tap: &RequestTap) -> String {
    let ticket = tap.begin(SITE, false);
    let id = ticket.id().to_owned();
    drop(ticket.record(
        RequestClock::start(),
        info(),
        200,
        Some("text/html"),
        http_body_util::Empty::<&'static [u8]>::new(),
    ));
    id
}

#[test]
fn mappers_are_field_for_field() {
    let p = yerd_proxy::ProxySample {
        id: "a".into(),
        site: "s".into(),
        php: "8.3".into(),
        method: "POST".into(),
        path: "/x".into(),
        script: Some("x.php".into()),
        status: 201,
        bytes: 3,
        content_type: Some("text/plain".into()),
        start_us: 4,
        duration_us: 5,
        aborted: true,
        proxy_error: true,
        traced: true,
    };
    let s = sample_from_proxy(p);
    assert_eq!(
        (
            s.id.as_str(),
            s.site.as_str(),
            s.php.as_str(),
            s.method.as_str()
        ),
        ("a", "s", "8.3", "POST")
    );
    assert_eq!(
        (s.path.as_str(), s.script.as_deref()),
        ("/x", Some("x.php"))
    );
    assert_eq!(
        (s.status, s.bytes, s.start_us, s.duration_us),
        (201, 3, 4, 5)
    );
    assert_eq!(s.content_type.as_deref(), Some("text/plain"));
    assert!(s.aborted && s.proxy_error && s.traced);

    let cfg = yerd_config::InsightsThresholds {
        n_plus_one_min: 1,
        duplicate_min: 2,
        slow_query_ms: 3,
        large_read_rows: 4,
        max_queries: 5,
    };
    let wire = thresholds_to_ipc(&cfg);
    assert_eq!(thresholds_from_ipc(&wire), cfg);
    let t = thresholds_from_config(&cfg);
    assert_eq!(
        (
            t.n_plus_one_min,
            t.duplicate_min,
            t.slow_query_ms,
            t.large_read_rows,
            t.max_queries
        ),
        (1, 2, 3, 4, 5)
    );
}

#[test]
fn tap_is_seeded_from_config() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = yerd_config::Config::default();
    cfg.insights.sites.insert("blog".into());
    cfg.dumps.port = 4321;
    cfg.dumps.features.insert("queries".into(), false);
    cfg.insights.thresholds.max_queries = 9;
    let ins = Insights::new(Store::new(tmp.path().to_path_buf()), 1, &cfg);
    assert!(ins.tap.is_enabled("blog"));
    assert!(!ins.tap.is_enabled("shop"));
    assert_eq!(ins.tap.port(), 4321);
    assert!(ins.tap.capture());
    assert!(!ins.tap.bound());
    assert_eq!(&*ins.tap.features(), "dumps,jobs,views,requests,logs,cache");
    assert_eq!(ins.agg.try_lock().unwrap().thresholds().max_queries, 9);
}

#[tokio::test]
async fn an_injected_sample_and_a_tick_count_in_the_overview() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_in(tmp.path());
    let now = now_ms();
    state
        .insights
        .agg
        .lock()
        .await
        .on_sample(sample("r1", now, false), now);
    tick_once(&state.insights, now, false, &mut TickState::default()).await;
    match overview(&state, SITE, InsightsRange::OneHour).await {
        Response::InsightsOverview { overview } => {
            assert_eq!(overview.kpis.requests, 1);
            assert!(!overview.ext_available);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn ingest_folds_samples_and_statics() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_in(tmp.path());
    let ins = &state.insights;
    let mut rx = ins.take_rx().unwrap();
    ins.tap.set_enabled_sites(["blog".to_owned()]);
    let id = record_through_tap(&ins.tap);
    ins.tap.note_static(SITE);
    drain_now(ins, &mut rx).await;
    let now = now_ms();
    let agg = ins.agg.lock().await;
    let rows = listed(&agg, now);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].content_type.as_deref(), Some("text/html"));
    assert_eq!(
        agg.overview(SITE, InsightsRange::OneHour, now)
            .kpis
            .static_skipped,
        1
    );
}

#[tokio::test]
async fn a_forced_flush_writes_files() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_in(tmp.path());
    let now = now_ms();
    feed_n_plus_one(&mut *state.insights.agg.lock().await, "r1", now);
    tick_once(&state.insights, now, true, &mut TickState::default()).await;
    let hour = hour_of(now);
    for kind in [FileKind::Roll, FileKind::Req, FileKind::Det] {
        assert!(
            site_dir(&state).join(file_name(hour, kind)).is_file(),
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn a_reload_reproduces_the_overview_and_open_issues() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("insights");
    let first = insights_at(&root);
    let now = now_ms();
    feed_n_plus_one(&mut *first.agg.lock().await, "r1", now);
    tick_once(&first, now, true, &mut TickState::default()).await;
    assert!(first.agg.lock().await.open_issue_count(SITE) > 0);

    let second = insights_at(&root);
    load(&second).await;
    let (a, b) = (first.agg.lock().await, second.agg.lock().await);
    for range in [InsightsRange::OneHour, InsightsRange::OneDay] {
        assert_eq!(b.overview(SITE, range, now), a.overview(SITE, range, now));
    }
    assert_eq!(b.open_issue_count(SITE), a.open_issue_count(SITE));
    assert_eq!(listed(&b, now).len(), 1);
}

#[tokio::test]
async fn shutdown_runs_the_final_flush() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_in(tmp.path());
    let ins = state.insights.clone();
    ins.tap.set_enabled_sites(["blog".to_owned()]);
    let (tx, rx) = watch::channel(false);
    let handle = spawn(ins.clone(), rx);
    record_through_tap(&ins.tap);
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .unwrap()
        .unwrap();
    let lines: usize = std::fs::read_dir(site_dir(&state))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "req"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap().lines().count())
        .sum();
    assert_eq!(lines, 1);
}

#[tokio::test]
async fn a_failing_store_counts_drops_and_recovers_the_dictionary() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("insights");
    std::fs::write(&root, b"not a dir").unwrap();
    let ins = insights_at(&root);
    let now = now_ms();
    ins.agg
        .lock()
        .await
        .on_sample(sample("r1", now, false), now);
    let mut st = TickState::default();
    tick_once(&ins, now, true, &mut st).await;
    {
        let agg = ins.agg.lock().await;
        assert_eq!(
            agg.overview(SITE, InsightsRange::OneHour, now)
                .dropped_requests,
            1
        );
        assert_eq!(listed(&agg, now).len(), 1);
    }
    tick_once(&ins, now + RECENT_MS + 1_000, false, &mut st).await;
    assert!(listed(&*ins.agg.lock().await, now + RECENT_MS + 1_000).is_empty());

    std::fs::remove_file(&root).unwrap();
    let later = now + RECENT_MS + 2_000;
    ins.agg
        .lock()
        .await
        .on_sample(sample("r2", later, false), later);
    tick_once(&ins, later, true, &mut st).await;
    let roll = std::fs::read_to_string(
        root.join(SITE)
            .join(file_name(hour_of(later), FileKind::Roll)),
    )
    .unwrap();
    assert!(roll.lines().any(|l| l.starts_with("{\"d\":")), "{roll}");
}

#[tokio::test]
async fn startup_load_orders_ingest_and_mutations_after_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = state_in(tmp.path());
    let root = state.insights.store.root().to_path_buf();
    let now = now_ms();
    let issue_key = {
        let seed = insights_at(&root);
        feed_n_plus_one(&mut *seed.agg.lock().await, "old", now);
        tick_once(&seed, now, true, &mut TickState::default()).await;
        let o = seed
            .agg
            .lock()
            .await
            .overview(SITE, InsightsRange::OneHour, now);
        o.issues[0].key.clone()
    };
    state.insights = Arc::new(insights_at(&root));
    let state = Arc::new(state);
    let ins = state.insights.clone();
    ins.tap.set_enabled_sites(["blog".to_owned()]);
    ins.tap.set_bound(true);

    let hold = ins.flushing.lock().await;
    let (tx, rx) = watch::channel(false);
    let handle = spawn(ins.clone(), rx);
    let ticket = ins.tap.begin(SITE, false);
    assert!(ticket.traced());
    let id = ticket.id().to_owned();
    {
        let mut agg = ins.agg.lock().await;
        agg.on_frame(query_frame(&id, 1, 1), now);
        agg.on_frame(summary_frame(&id, 2, 2, now), now);
        assert_eq!(agg.sites().count(), 0);
    }
    drop(ticket.record(
        RequestClock::start(),
        info(),
        200,
        None,
        http_body_util::Empty::<&'static [u8]>::new(),
    ));
    let dismiss_task = {
        let state = state.clone();
        let key = issue_key.clone();
        tokio::spawn(async move { dismiss(&state, SITE, &key, true).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!dismiss_task.is_finished());
    drop(hold);

    assert!(matches!(dismiss_task.await.unwrap(), Response::Ok));
    wait_for(|| {
        ins.agg
            .try_lock()
            .is_ok_and(|agg| listed(&agg, now_ms()).len() == 2)
    })
    .await;
    {
        let agg = ins.agg.lock().await;
        let rows = listed(&agg, now_ms());
        let joined = rows.iter().find(|r| r.id == id).unwrap();
        assert_eq!(joined.queries, 1);
        assert!(rows.iter().any(|r| r.id == "old"));
        let key = yerd_insights::pure::model::parse_key_hex(&issue_key).unwrap();
        assert_eq!(agg.dismissed_keys(SITE), vec![key]);
    }
    assert_eq!(
        ins.store
            .load_dismissed(SITE)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![yerd_insights::pure::model::parse_key_hex(&issue_key).unwrap()]
    );
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_cap_prune_from_the_tick_forgets_the_written_hour() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("insights");
    let mut ins = Insights::new(
        Store::with_cap(root.clone(), 1),
        7,
        &yerd_config::Config::default(),
    );
    ins.prune_check_bytes = 1;
    let now = now_ms();
    let mut st = TickState {
        bytes_since_prune: 0,
        last_prune_hour: Some(hour_of(now)),
    };
    ins.agg
        .lock()
        .await
        .on_sample(sample("r1", now, false), now);
    tick_once(&ins, now, true, &mut st).await;
    assert_eq!(st.bytes_since_prune, 0);
    let roll = root
        .join(SITE)
        .join(file_name(hour_of(now), FileKind::Roll));
    assert!(!roll.exists());

    ins.prune_check_bytes = u64::MAX;
    ins.agg
        .lock()
        .await
        .on_sample(sample("r2", now, false), now);
    tick_once(&ins, now, true, &mut st).await;
    let text = std::fs::read_to_string(&roll).unwrap();
    assert!(text.lines().any(|l| l.starts_with("{\"d\":")), "{text}");
}

#[tokio::test]
async fn enabling_persists_and_updates_the_tap() {
    let tmp = tempfile::tempdir().unwrap();
    let state = Arc::new(state_in(tmp.path()));
    let resp = crate::ipc_server::respond(
        Request::SetInsightsEnabled {
            site: "Blog".into(),
            enabled: true,
        },
        &state,
    )
    .await;
    assert!(matches!(resp, Response::Ok), "{resp:?}");
    assert!(state.config.lock().await.insights.sites.contains("blog"));
    assert!(state.insights.tap.is_enabled("blog"));
    let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
    assert!(reloaded.insights.sites.contains("blog"));
    match status(&state).await {
        Response::InsightsStatus {
            sites,
            capture_enabled,
            open_issues,
            ..
        } => {
            assert_eq!(sites, vec!["blog".to_owned()]);
            assert!(capture_enabled);
            assert_eq!(open_issues, 0);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        set_enabled(&state, " ", true).await,
        Response::Error {
            code: ErrorCode::NotFound,
            ..
        }
    ));
}

#[tokio::test]
async fn disabling_flushes_the_site() {
    let tmp = tempfile::tempdir().unwrap();
    let state = Arc::new(state_in(tmp.path()));
    set_enabled(&state, SITE, true).await;
    let now = now_ms();
    state
        .insights
        .agg
        .lock()
        .await
        .on_sample(sample("r1", now, false), now);
    assert!(matches!(
        set_enabled(&state, SITE, false).await,
        Response::Ok
    ));
    assert!(!state.insights.tap.is_enabled(SITE));
    assert!(state.config.lock().await.insights.sites.is_empty());
    assert!(site_dir(&state)
        .join(file_name(hour_of(now), FileKind::Req))
        .is_file());
}

#[tokio::test]
async fn read_handlers_on_an_unknown_site_are_empty_and_create_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_in(tmp.path());
    let site = Site::linked(SITE, "/srv/www/blog", PhpVersion::new(8, 3)).unwrap();
    state.router.write().await.insert(site).unwrap();
    match overview(&state, SITE, InsightsRange::OneDay).await {
        Response::InsightsOverview { overview } => {
            assert_eq!(overview.kpis.requests, 0);
            assert!(overview.series.iter().all(|p| p.count == 0));
            assert!(overview.ext_available);
        }
        other => panic!("{other:?}"),
    }
    for range in [InsightsRange::FiveMinutes, InsightsRange::OneDay] {
        match list_requests(&state, SITE, range, None, InsightsSort::Recent, 50).await {
            Response::InsightsRequests { requests } => assert!(requests.is_empty()),
            other => panic!("{other:?}"),
        }
    }
    assert!(matches!(
        get_request(&state, SITE, "nope", now_ms()).await,
        Response::Error {
            code: ErrorCode::NotFound,
            ..
        }
    ));
    assert!(matches!(
        status(&state).await,
        Response::InsightsStatus { open_issues: 0, .. }
    ));
    assert_eq!(state.insights.agg.lock().await.sites().count(), 0);
}

#[tokio::test]
async fn legacy_php_sites_are_not_ext_available() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_in(tmp.path());
    let site = Site::linked(SITE, "/srv/www/blog", PhpVersion::new(8, 1)).unwrap();
    state.router.write().await.insert(site).unwrap();
    match overview(&state, SITE, InsightsRange::OneHour).await {
        Response::InsightsOverview { overview } => assert!(!overview.ext_available),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn list_merges_memory_and_disk_and_detail_falls_back_to_the_store() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_in(tmp.path());
    let now = now_ms();
    feed_n_plus_one(&mut *state.insights.agg.lock().await, "r1", now);
    tick_once(&state.insights, now, true, &mut TickState::default()).await;
    feed_n_plus_one(&mut *state.insights.agg.lock().await, "r2", now);
    assert!(state
        .insights
        .agg
        .lock()
        .await
        .recent_detail(SITE, "r1")
        .is_none());

    match list_requests(
        &state,
        SITE,
        InsightsRange::OneHour,
        None,
        InsightsSort::Recent,
        50,
    )
    .await
    {
        Response::InsightsRequests { requests } => {
            let mut ids: Vec<&str> = requests.iter().map(|r| r.id.as_str()).collect();
            ids.sort_unstable();
            assert_eq!(ids, vec!["r1", "r2"]);
        }
        other => panic!("{other:?}"),
    }
    match list_requests(
        &state,
        SITE,
        InsightsRange::OneHour,
        Some("POST".into()),
        InsightsSort::Slowest,
        10,
    )
    .await
    {
        Response::InsightsRequests { requests } => assert!(requests.is_empty()),
        other => panic!("{other:?}"),
    }
    for id in ["r1", "r2"] {
        match get_request(&state, SITE, id, now).await {
            Response::InsightsRequest { request } => {
                assert_eq!(request.summary.id, id);
                assert_eq!(request.events.len(), 6);
            }
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn clear_removes_files_and_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_in(tmp.path());
    let now = now_ms();
    state
        .insights
        .agg
        .lock()
        .await
        .on_sample(sample("r1", now, false), now);
    tick_once(&state.insights, now, true, &mut TickState::default()).await;
    assert!(site_dir(&state).is_dir());
    assert!(matches!(clear(&state, SITE).await, Response::Ok));
    assert!(!site_dir(&state).exists());
    assert!(listed(&*state.insights.agg.lock().await, now).is_empty());
}

#[tokio::test]
async fn clear_waits_for_an_in_flight_flush() {
    let tmp = tempfile::tempdir().unwrap();
    let state = Arc::new(state_in(tmp.path()));
    let ins = state.insights.clone();
    let now = now_ms();
    ins.agg
        .lock()
        .await
        .on_sample(sample("r1", now, false), now);

    let hold = ins.flushing.lock().await;
    let batches = ins.agg.lock().await.take_flush(now, true, None);
    assert_eq!(batches.len(), 1);
    let clear_task = {
        let state = state.clone();
        tokio::spawn(async move { clear(&state, SITE).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!clear_task.is_finished());
    write_batches(&ins, batches, now).await;
    assert!(site_dir(&state).is_dir());
    drop(hold);
    assert!(matches!(clear_task.await.unwrap(), Response::Ok));
    assert!(!site_dir(&state).exists());
    assert!(listed(&*ins.agg.lock().await, now).is_empty());
}

#[tokio::test]
async fn stale_confirm_and_failure_after_a_clear_are_no_ops() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_in(tmp.path());
    let ins = &state.insights;
    let now = now_ms();
    ins.agg
        .lock()
        .await
        .on_sample(sample("r1", now, false), now);
    let batch = ins.agg.lock().await.take_flush(now, true, None).remove(0);
    let mut agg = ins.agg.lock().await;
    agg.clear(SITE, now);
    agg.confirm_flush(SITE, batch.generation, &batch.ids());
    agg.flush_failed(SITE, batch.generation, &batch.ids(), &batch.hours(), now);
    assert!(listed(&agg, now).is_empty());
    assert_eq!(
        agg.overview(SITE, InsightsRange::OneHour, now)
            .dropped_requests,
        0
    );
    drop(agg);
    assert!(!site_dir(&state).exists());
}

#[tokio::test]
async fn thresholds_persist_and_validate() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_in(tmp.path());
    let mut t = yerd_ipc::InsightsThresholds {
        duplicate_min: 1,
        ..Default::default()
    };
    assert!(matches!(
        set_thresholds(&state, &t).await,
        Response::Error {
            code: ErrorCode::InvalidSetting,
            ..
        }
    ));
    t.duplicate_min = 3;
    t.max_queries = 7;
    assert!(matches!(set_thresholds(&state, &t).await, Response::Ok));
    assert_eq!(state.config.lock().await.insights.thresholds.max_queries, 7);
    let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
    assert_eq!(reloaded.insights.thresholds.duplicate_min, 3);
    assert_eq!(state.insights.agg.lock().await.thresholds().max_queries, 7);
    match status(&state).await {
        Response::InsightsStatus { thresholds, .. } => assert_eq!(thresholds, t),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn dismiss_rejects_bad_keys_and_persists_good_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_in(tmp.path());
    assert!(matches!(
        dismiss(&state, SITE, "zz", true).await,
        Response::Error {
            code: ErrorCode::InvalidSetting,
            ..
        }
    ));
    let now = now_ms();
    feed_n_plus_one(&mut *state.insights.agg.lock().await, "r1", now);
    let key = state
        .insights
        .agg
        .lock()
        .await
        .overview(SITE, InsightsRange::OneHour, now)
        .issues[0]
        .key
        .clone();
    assert!(matches!(
        dismiss(&state, SITE, &key, true).await,
        Response::Ok
    ));
    assert_eq!(state.insights.store.load_dismissed(SITE).unwrap().len(), 1);
    assert_eq!(state.insights.agg.lock().await.open_issue_count(SITE), 0);
    assert!(matches!(
        dismiss(&state, SITE, &key, false).await,
        Response::Ok
    ));
    assert!(state
        .insights
        .store
        .load_dismissed(SITE)
        .unwrap()
        .is_empty());
}
