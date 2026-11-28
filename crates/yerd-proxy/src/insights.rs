//! Insights request tap: per-site request timing handed to the daemon.
//!
//! For a site with Insights enabled the proxy mints a request id, optionally
//! passes the `YERD_TRACE_*` FastCGI params to the extension, and sends one
//! [`ProxySample`] when the response body ends (or is abandoned). Everything
//! leaves through a bounded channel with `try_send`: a full channel drops
//! the sample and counts it, and the request path never waits on the daemon.
//!
//! The cost for a site that is not enabled is one atomic load while no site
//! is enabled, plus an uncontended read-lock set lookup while some site is.
//! Nothing is allocated on that path.
//!
//! This crate does not depend on `yerd-insights`: [`ProxySample`] is its own
//! type and the daemon maps it field for field.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::task::{Context, Poll};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Buf;
use http_body::{Body, Frame, SizeHint};
use tokio::sync::mpsc;

use crate::pure::cgi_params::TraceParams;

/// Default bound of the tap channel.
pub const DEFAULT_CHANNEL_CAPACITY: usize = 16_384;

/// Longest `content_type` kept on a sample, in bytes.
pub const MAX_CONTENT_TYPE_BYTES: usize = 64;

/// One PHP-FPM request as the proxy saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySample {
    /// The request id minted by [`RequestTap::begin`].
    pub id: String,
    /// The Yerd site name.
    pub site: String,
    /// The site's PHP version, e.g. `8.3`.
    pub php: String,
    /// The HTTP method.
    pub method: String,
    /// The request path and query, as forwarded to PHP.
    pub path: String,
    /// The real script executed, relative to the served root, or `None` for
    /// the front controller.
    pub script: Option<String>,
    /// The response status, `500` for a proxy error.
    pub status: u16,
    /// Response body bytes handed to the client.
    pub bytes: u64,
    /// The response `Content-Type`, clipped to [`MAX_CONTENT_TYPE_BYTES`].
    pub content_type: Option<String>,
    /// Wall-clock Unix epoch microseconds when the proxy started the request.
    pub start_us: u64,
    /// Microseconds from the start of the request to the end of the body.
    pub duration_us: u64,
    /// Whether the body was dropped before it ended (client went away).
    pub aborted: bool,
    /// Whether the proxy failed to get a response from PHP-FPM at all.
    pub proxy_error: bool,
    /// Whether the `YERD_TRACE_*` params were sent with the request.
    pub traced: bool,
}

/// A message from the proxy to the daemon's Insights drain task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapMessage {
    /// A PHP-FPM request finished.
    Sample(ProxySample),
    /// A static file was served for an enabled site instead of PHP.
    Static {
        /// The Yerd site name.
        site: String,
        /// Wall-clock Unix epoch milliseconds.
        ts_ms: u64,
    },
}

/// The channel end shared by the tap and every in-flight [`RecordingBody`].
#[derive(Debug)]
struct Sink {
    tx: mpsc::Sender<TapMessage>,
    dropped: Mutex<HashMap<String, u64>>,
}

impl Sink {
    fn send_sample(&self, sample: ProxySample) {
        let Err(err) = self.tx.try_send(TapMessage::Sample(sample)) else {
            return;
        };
        let (mpsc::error::TrySendError::Full(msg) | mpsc::error::TrySendError::Closed(msg)) = err;
        let site = match msg {
            TapMessage::Sample(sample) => sample.site,
            TapMessage::Static { site, .. } => site,
        };
        let mut dropped = self.dropped.lock().unwrap_or_else(PoisonError::into_inner);
        let count = dropped.entry(site).or_insert(0);
        *count = count.saturating_add(1);
    }
}

/// Per-site request recorder shared between the daemon and the proxy.
///
/// The daemon keeps the setters in step with its config and the dump
/// server; the proxy reaches it through
/// [`crate::BackendResolver::request_tap`].
#[derive(Debug)]
pub struct RequestTap {
    sink: Arc<Sink>,
    any_enabled: AtomicBool,
    enabled: RwLock<HashSet<String>>,
    nonce: u64,
    counter: AtomicU64,
    port: AtomicU16,
    capture: AtomicBool,
    bound: AtomicBool,
    features: RwLock<Arc<str>>,
}

impl RequestTap {
    /// A tap with no site enabled, capture off and the dump server unbound,
    /// plus the receiving end of its channel. `capacity` is clamped to at
    /// least 1. `nonce` is the daemon boot nonce that prefixes every id.
    pub fn new(capacity: usize, nonce: u64) -> (Arc<Self>, mpsc::Receiver<TapMessage>) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        let tap = Self {
            sink: Arc::new(Sink {
                tx,
                dropped: Mutex::new(HashMap::new()),
            }),
            any_enabled: AtomicBool::new(false),
            enabled: RwLock::new(HashSet::new()),
            nonce,
            counter: AtomicU64::new(0),
            port: AtomicU16::new(0),
            capture: AtomicBool::new(false),
            bound: AtomicBool::new(false),
            features: RwLock::new(Arc::from("")),
        };
        (Arc::new(tap), rx)
    }

    /// Replace the set of Insights-enabled site names (lowercased).
    pub fn set_enabled_sites<I: IntoIterator<Item = String>>(&self, sites: I) {
        let next: HashSet<String> = sites.into_iter().collect();
        let any = !next.is_empty();
        *self.enabled.write().unwrap_or_else(PoisonError::into_inner) = next;
        self.any_enabled.store(any, Ordering::Release);
    }

    /// Set the dump-server loopback port passed as `YERD_TRACE_PORT`.
    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::Relaxed);
    }

    /// Set the global extension-capture switch (`dumps.enabled`).
    pub fn set_capture(&self, capture: bool) {
        self.capture.store(capture, Ordering::Relaxed);
    }

    /// Record whether the dump server is currently bound.
    pub fn set_bound(&self, bound: bool) {
        self.bound.store(bound, Ordering::Relaxed);
    }

    /// Set the comma-separated feature list passed as `YERD_TRACE_FEATURES`.
    pub fn set_features(&self, features: &str) {
        *self
            .features
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Arc::from(features);
    }

    /// The current dump-server port.
    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    /// The current capture switch.
    pub fn capture(&self) -> bool {
        self.capture.load(Ordering::Relaxed)
    }

    /// Whether the dump server is recorded as bound.
    pub fn bound(&self) -> bool {
        self.bound.load(Ordering::Relaxed)
    }

    /// The current feature list.
    pub fn features(&self) -> Arc<str> {
        Arc::clone(&self.features.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Whether `site` has Insights enabled. Lock-free and allocation-free
    /// while no site is enabled.
    pub fn is_enabled(&self, site: &str) -> bool {
        if !self.any_enabled.load(Ordering::Acquire) {
            return false;
        }
        self.enabled
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(site)
    }

    /// Start recording one PHP-FPM request for `site`. The ticket carries
    /// trace params only when capture is on, the dump server is bound with a
    /// port, and the site's PHP version is not `legacy` (no extension build).
    pub fn begin(&self, site: &str, legacy: bool) -> Ticket {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let port = self.port();
        let traced = !legacy && port != 0 && self.capture() && self.bound();
        Ticket {
            sink: Arc::clone(&self.sink),
            id: format!("{:016x}{n:016x}", self.nonce),
            site: site.to_owned(),
            traced,
            port,
            features: if traced {
                self.features()
            } else {
                Arc::from("")
            },
        }
    }

    /// Note a static file answered for `site`. Never blocks; a full channel
    /// drops the note without counting it, since it is not a request sample.
    pub fn note_static(&self, site: &str) {
        let _ = self.sink.tx.try_send(TapMessage::Static {
            site: site.to_owned(),
            ts_ms: epoch_us() / 1_000,
        });
    }

    /// Take and reset the per-site count of samples lost to a full or closed
    /// channel.
    pub fn take_dropped(&self) -> HashMap<String, u64> {
        std::mem::take(
            &mut *self
                .sink
                .dropped
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }
}

/// When a recorded request started: a monotonic clock for the duration and
/// the wall clock for `start_us`.
#[derive(Debug, Clone, Copy)]
pub struct RequestClock {
    started: Instant,
    start_us: u64,
}

impl RequestClock {
    /// Read both clocks now.
    pub fn start() -> Self {
        Self {
            started: Instant::now(),
            start_us: epoch_us(),
        }
    }

    /// Wall-clock Unix epoch microseconds at the start.
    pub fn start_us(&self) -> u64 {
        self.start_us
    }

    fn elapsed_us(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX)
    }
}

/// The request facts a sample carries besides its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestInfo {
    /// The site's PHP version, e.g. `8.3`.
    pub php: String,
    /// The HTTP method.
    pub method: String,
    /// The request path and query, as forwarded to PHP.
    pub path: String,
    /// The real script executed, or `None` for the front controller.
    pub script: Option<String>,
}

/// One request being recorded, from [`RequestTap::begin`]. Consumed by
/// [`Ticket::record`] or [`Ticket::proxy_error`]; dropping it records nothing.
#[derive(Debug)]
pub struct Ticket {
    sink: Arc<Sink>,
    id: String,
    site: String,
    traced: bool,
    port: u16,
    features: Arc<str>,
}

impl Ticket {
    /// The minted request id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Whether the trace params are sent with this request.
    pub fn traced(&self) -> bool {
        self.traced
    }

    /// The `YERD_TRACE_*` params, or `None` when the request is timing-only.
    pub fn trace_params(&self) -> Option<TraceParams<'_>> {
        self.traced.then(|| TraceParams {
            id: &self.id,
            port: self.port,
            site: &self.site,
            features: &self.features,
        })
    }

    /// Wrap a response `body` so its sample is sent once the body ends or is
    /// dropped. A body already at its end (HEAD, 204, 304) is recorded at
    /// once as completed.
    pub fn record<B>(
        self,
        clock: RequestClock,
        info: RequestInfo,
        status: u16,
        content_type: Option<&str>,
        body: B,
    ) -> RecordingBody<B>
    where
        B: Body + Unpin,
    {
        let sample = self.sample(&clock, info, status, content_type.map(clip_content_type));
        let mut recording = RecordingBody {
            inner: body,
            sink: self.sink,
            clock,
            bytes: 0,
            sample: Some(sample),
        };
        if recording.inner.is_end_stream() {
            recording.emit(false);
        }
        recording
    }

    /// Send a sample at once for a request PHP-FPM never answered: status
    /// `500`, `proxy_error`, and never traced since PHP did not run it.
    pub fn proxy_error(self, clock: RequestClock, info: RequestInfo) {
        let mut sample = self.sample(&clock, info, 500, None);
        sample.proxy_error = true;
        sample.traced = false;
        sample.duration_us = clock.elapsed_us();
        self.sink.send_sample(sample);
    }

    fn sample(
        &self,
        clock: &RequestClock,
        info: RequestInfo,
        status: u16,
        content_type: Option<String>,
    ) -> ProxySample {
        ProxySample {
            id: self.id.clone(),
            site: self.site.clone(),
            php: info.php,
            method: info.method,
            path: info.path,
            script: info.script,
            status,
            bytes: 0,
            content_type,
            start_us: clock.start_us,
            duration_us: 0,
            aborted: false,
            proxy_error: false,
            traced: self.traced,
        }
    }
}

/// A response body that counts data bytes and sends its [`ProxySample`]
/// exactly once: on the end of the stream (completed), or on drop (completed
/// if the inner body had reached its end, aborted otherwise).
#[derive(Debug)]
pub struct RecordingBody<B: Body + Unpin> {
    inner: B,
    sink: Arc<Sink>,
    clock: RequestClock,
    bytes: u64,
    sample: Option<ProxySample>,
}

impl<B: Body + Unpin> RecordingBody<B> {
    fn emit(&mut self, aborted: bool) {
        let Some(mut sample) = self.sample.take() else {
            return;
        };
        sample.bytes = self.bytes;
        sample.duration_us = self.clock.elapsed_us();
        sample.aborted = aborted;
        self.sink.send_sample(sample);
    }
}

impl<B> Body for RecordingBody<B>
where
    B: Body + Unpin,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    let len = u64::try_from(data.remaining()).unwrap_or(u64::MAX);
                    this.bytes = this.bytes.saturating_add(len);
                }
                if this.inner.is_end_stream() {
                    this.emit(false);
                }
            }
            Poll::Ready(None) => this.emit(false),
            Poll::Ready(Some(Err(_))) | Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl<B: Body + Unpin> Drop for RecordingBody<B> {
    fn drop(&mut self) {
        if self.sample.is_some() {
            let aborted = !self.inner.is_end_stream();
            self.emit(aborted);
        }
    }
}

fn clip_content_type(value: &str) -> String {
    let mut end = value.len().min(MAX_CONTENT_TYPE_BYTES);
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value.get(..end).unwrap_or_default().to_owned()
}

fn epoch_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty, Full};
    use std::collections::VecDeque;
    use std::convert::Infallible;

    const NONCE: u64 = 0x0123_4567_89ab_cdef;

    fn enabled_tap(capacity: usize) -> (Arc<RequestTap>, mpsc::Receiver<TapMessage>) {
        let (tap, rx) = RequestTap::new(capacity, NONCE);
        tap.set_enabled_sites(["shop".to_owned()]);
        tap.set_port(9912);
        tap.set_capture(true);
        tap.set_bound(true);
        tap.set_features("dumps,queries");
        (tap, rx)
    }

    fn info() -> RequestInfo {
        RequestInfo {
            php: "8.3".to_owned(),
            method: "GET".to_owned(),
            path: "/orders/1042?tab=items".to_owned(),
            script: None,
        }
    }

    fn sample_of(msg: TapMessage) -> ProxySample {
        match msg {
            TapMessage::Sample(sample) => sample,
            TapMessage::Static { .. } => panic!("expected a sample, got {msg:?}"),
        }
    }

    struct Frames {
        frames: VecDeque<Bytes>,
    }

    impl Body for Frames {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            Poll::Ready(
                self.get_mut()
                    .frames
                    .pop_front()
                    .map(|b| Ok(Frame::data(b))),
            )
        }
    }

    struct Stalled;

    impl Body for Stalled {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            Poll::Pending
        }
    }

    #[test]
    fn nothing_enabled_is_the_lock_free_fast_path() {
        let (tap, _rx) = RequestTap::new(4, NONCE);
        assert!(!tap.is_enabled("shop"));
        let _guard = tap.enabled.write().unwrap();
        assert!(!tap.is_enabled("shop"));
    }

    #[test]
    fn enabled_set_is_replaced_and_cleared() {
        let (tap, _rx) = RequestTap::new(4, NONCE);
        tap.set_enabled_sites(["shop".to_owned(), "blog".to_owned()]);
        assert!(tap.is_enabled("shop"));
        assert!(tap.is_enabled("blog"));
        assert!(!tap.is_enabled("api"));
        tap.set_enabled_sites(Vec::new());
        assert!(!tap.any_enabled.load(Ordering::Acquire));
        assert!(!tap.is_enabled("shop"));
    }

    #[test]
    fn begin_ids_are_32_lowercase_hex_nonce_then_counter_and_unique() {
        let (tap, _rx) = enabled_tap(4);
        let ids: Vec<String> = (0..3)
            .map(|_| tap.begin("shop", false).id().to_owned())
            .collect();
        for id in &ids {
            assert_eq!(id.len(), 32);
            assert!(id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            assert!(id.starts_with("0123456789abcdef"));
        }
        assert_eq!(ids[0], "0123456789abcdef0000000000000000");
        assert_eq!(ids[2], "0123456789abcdef0000000000000002");
        let unique: HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len());
    }

    #[test]
    fn traced_ticket_carries_all_four_params() {
        let (tap, _rx) = enabled_tap(4);
        let ticket = tap.begin("shop", false);
        assert!(ticket.traced());
        assert_eq!(
            ticket.trace_params(),
            Some(TraceParams {
                id: ticket.id(),
                port: 9912,
                site: "shop",
                features: "dumps,queries",
            })
        );
    }

    #[test]
    fn untraced_conditions_yield_no_params() {
        type Setup = fn(&RequestTap);
        let cases: [(&str, Setup, bool); 4] = [
            ("capture off", |t| t.set_capture(false), false),
            ("dump server unbound", |t| t.set_bound(false), false),
            ("port zero", |t| t.set_port(0), false),
            ("legacy php", |_| {}, true),
        ];
        for (name, setup, legacy) in cases {
            let (tap, _rx) = enabled_tap(4);
            setup(&tap);
            let ticket = tap.begin("shop", legacy);
            assert!(!ticket.traced(), "{name}");
            assert!(ticket.trace_params().is_none(), "{name}");
        }
    }

    #[tokio::test]
    async fn completed_body_emits_once_with_bytes_and_metadata() {
        let (tap, mut rx) = enabled_tap(4);
        let body = Frames {
            frames: VecDeque::from([Bytes::from_static(b"hello "), Bytes::from_static(b"world")]),
        };
        let clock = RequestClock::start();
        let recording = tap.begin("shop", false).record(
            clock,
            info(),
            200,
            Some("text/html; charset=utf-8"),
            body,
        );
        let collected = recording.collect().await.unwrap().to_bytes();
        assert_eq!(collected.as_ref(), b"hello world");
        let sample = sample_of(rx.try_recv().unwrap());
        assert!(rx.try_recv().is_err());
        assert_eq!(sample.site, "shop");
        assert_eq!(sample.php, "8.3");
        assert_eq!(sample.method, "GET");
        assert_eq!(sample.path, "/orders/1042?tab=items");
        assert_eq!(sample.status, 200);
        assert_eq!(sample.bytes, 11);
        assert_eq!(
            sample.content_type.as_deref(),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(sample.start_us, clock.start_us());
        assert!(!sample.aborted);
        assert!(!sample.proxy_error);
        assert!(sample.traced);
    }

    #[tokio::test]
    async fn full_body_emits_when_inner_reports_its_end() {
        let (tap, mut rx) = enabled_tap(4);
        let mut recording = tap.begin("shop", false).record(
            RequestClock::start(),
            info(),
            200,
            None,
            Full::new(Bytes::from_static(b"abc")),
        );
        let frame = recording.frame().await.unwrap().unwrap();
        assert_eq!(frame.into_data().unwrap().as_ref(), b"abc");
        let sample = sample_of(rx.try_recv().unwrap());
        assert_eq!(sample.bytes, 3);
        assert!(!sample.aborted);
        drop(recording);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn early_drop_emits_once_as_aborted() {
        let (tap, mut rx) = enabled_tap(4);
        let recording =
            tap.begin("shop", false)
                .record(RequestClock::start(), info(), 200, None, Stalled);
        assert!(rx.try_recv().is_err());
        drop(recording);
        let sample = sample_of(rx.try_recv().unwrap());
        assert!(sample.aborted);
        assert_eq!(sample.bytes, 0);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn bodyless_response_is_recorded_as_completed_at_wrap_time() {
        let (tap, mut rx) = enabled_tap(4);
        let recording = tap.begin("shop", false).record(
            RequestClock::start(),
            info(),
            204,
            None,
            Empty::<Bytes>::new(),
        );
        let sample = sample_of(rx.try_recv().unwrap());
        assert_eq!(sample.status, 204);
        assert!(!sample.aborted);
        drop(recording);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn full_channel_counts_a_drop_without_blocking() {
        let (tap, mut rx) = enabled_tap(1);
        for _ in 0..3 {
            let _ = tap.begin("shop", false).record(
                RequestClock::start(),
                info(),
                200,
                None,
                Empty::<Bytes>::new(),
            );
        }
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
        assert_eq!(tap.take_dropped(), HashMap::from([("shop".to_owned(), 2)]));
        assert!(tap.take_dropped().is_empty());
    }

    #[test]
    fn proxy_error_sends_an_untraced_500_at_once() {
        let (tap, mut rx) = enabled_tap(4);
        let ticket = tap.begin("shop", false);
        assert!(ticket.traced());
        ticket.proxy_error(RequestClock::start(), info());
        let sample = sample_of(rx.try_recv().unwrap());
        assert_eq!(sample.status, 500);
        assert!(sample.proxy_error);
        assert!(!sample.traced);
        assert!(!sample.aborted);
    }

    #[test]
    fn note_static_sends_a_static_message() {
        let (tap, mut rx) = enabled_tap(4);
        tap.note_static("shop");
        match rx.try_recv().unwrap() {
            TapMessage::Static { site, ts_ms } => {
                assert_eq!(site, "shop");
                assert!(ts_ms > 0);
            }
            other @ TapMessage::Sample(_) => panic!("expected static, got {other:?}"),
        }
    }

    #[test]
    fn content_type_is_clipped_on_a_char_boundary() {
        let cases = [
            ("text/html", "text/html".to_owned()),
            (&*"a".repeat(80), "a".repeat(64)),
            (&*format!("{}é", "a".repeat(63)), "a".repeat(63)),
        ];
        for (input, expected) in cases {
            assert_eq!(clip_content_type(input), expected, "{input}");
        }
    }

    #[test]
    fn setters_are_readable() {
        let (tap, _rx) = RequestTap::new(4, NONCE);
        tap.set_port(4321);
        tap.set_capture(true);
        tap.set_bound(true);
        tap.set_features("logs");
        assert_eq!(tap.port(), 4321);
        assert!(tap.capture());
        assert!(tap.bound());
        assert_eq!(&*tap.features(), "logs");
    }
}
