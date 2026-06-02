//! End-to-end smoke test: bind a real `Bound` on `127.0.0.1:0`, drive it via
//! `hickory-client`, assert the wire shape of every Answer variant + that
//! shutdown completes within a bounded timeout.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

use hickory_client::client::{AsyncClient, ClientHandle};
use hickory_client::error::ClientError;
use hickory_proto::iocompat::AsyncIoTokioAsStd;
use hickory_proto::op::ResponseCode;
use hickory_proto::rr::{rdata, DNSClass, Name, RData, RecordType};

use yerd_core::Tld;
use yerd_dns::{Bound, Responder, ANSWER_TTL_SECS};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn serves_default_tld() {
    serve_and_query(Tld::new("test").unwrap(), "app.test.", "test.").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn serves_multi_label_tld() {
    serve_and_query(
        Tld::new("dev.local").unwrap(),
        "app.dev.local.",
        "dev.local.",
    )
    .await;
}

async fn serve_and_query(tld: Tld, site_fqdn: &str, apex_fqdn: &str) {
    let bound = Bound::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = bound.local_addr();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(bound.serve(Responder::new(tld), async move {
        let _ = rx.await;
    }));

    // --- UDP client -----------------------------------------------------
    let conn = hickory_client::udp::UdpClientStream::<tokio::net::UdpSocket>::new(addr);
    let (mut udp_client, bg) = AsyncClient::connect(conn).await.unwrap();
    tokio::spawn(bg);

    // 1. $site A → Loopback4
    let site = Name::from_str(site_fqdn).unwrap();
    let resp = timeout(udp_client.query(site.clone(), DNSClass::IN, RecordType::A)).await;
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert_eq!(resp.answers().len(), 1);
    assert_eq!(resp.answers()[0].ttl(), ANSWER_TTL_SECS);
    match resp.answers()[0].data() {
        Some(RData::A(rdata::A(ip))) => assert_eq!(*ip, Ipv4Addr::LOCALHOST),
        other => panic!("expected RData::A, got {other:?}"),
    }

    // 2. $site AAAA → Loopback6
    let resp = timeout(udp_client.query(site.clone(), DNSClass::IN, RecordType::AAAA)).await;
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert_eq!(resp.answers().len(), 1);
    match resp.answers()[0].data() {
        Some(RData::AAAA(rdata::AAAA(ip))) => assert_eq!(*ip, Ipv6Addr::LOCALHOST),
        other => panic!("expected RData::AAAA, got {other:?}"),
    }

    // 3. $site MX → NoData (NOERROR + empty answer + no SOA in authority).
    //    Pins the RFC 8020 §2 / RFC 2308 §3 decision from lib.rs.
    let resp = timeout(udp_client.query(site.clone(), DNSClass::IN, RecordType::MX)).await;
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert_eq!(resp.answers().len(), 0);
    assert_eq!(resp.name_servers().len(), 0);

    // 4. unrelated.com A → REFUSED, non-authoritative (out of our zone).
    let unrelated = Name::from_str("unrelated.com.").unwrap();
    let resp = timeout(udp_client.query(unrelated, DNSClass::IN, RecordType::A)).await;
    assert_eq!(resp.response_code(), ResponseCode::Refused);
    assert!(
        !resp.authoritative(),
        "out-of-zone reply must clear the AA bit"
    );

    // 5. $apex A → NoData (apex carve-out).
    let apex = Name::from_str(apex_fqdn).unwrap();
    let resp = timeout(udp_client.query(apex, DNSClass::IN, RecordType::A)).await;
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert_eq!(resp.answers().len(), 0);

    // --- TCP client to exercise the TCP listener path -------------------
    let (tcp_conn, sender) =
        hickory_client::tcp::TcpClientStream::<AsyncIoTokioAsStd<tokio::net::TcpStream>>::new(addr);
    let (mut tcp_client, bg) = AsyncClient::new(tcp_conn, sender, None).await.unwrap();
    tokio::spawn(bg);
    let resp = timeout(tcp_client.query(site, DNSClass::IN, RecordType::A)).await;
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert_eq!(resp.answers().len(), 1);
    match resp.answers()[0].data() {
        Some(RData::A(rdata::A(ip))) => assert_eq!(*ip, Ipv4Addr::LOCALHOST),
        other => panic!("expected RData::A over TCP, got {other:?}"),
    }

    // --- Malformed UDP packet: QDCOUNT=0 --------------------------------
    // Hickory's parser pre-handler FORMERRs this; our LoopbackHandler is
    // never invoked. Pins:
    //   - hickory echoes the request ID rather than synthesising a fresh header
    //   - RCode = FORMERR (1)
    {
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sock.connect(addr).await.unwrap();
        // 12-byte DNS header: id=0xCAFE, flags 0x0100 (RD), QDCOUNT=0,
        // ANCOUNT=0, NSCOUNT=0, ARCOUNT=0.
        let probe: [u8; 12] = [0xCA, 0xFE, 0x01, 0x00, 0, 0, 0, 0, 0, 0, 0, 0];
        sock.send(&probe).await.unwrap();
        let mut buf = [0u8; 512];
        let n = tokio::time::timeout(Duration::from_secs(2), sock.recv(&mut buf))
            .await
            .expect("malformed-packet reply timed out")
            .unwrap();
        assert!(n >= 12, "FORMERR reply must include a DNS header");
        assert_eq!(buf[0], 0xCA, "request ID byte 0 not echoed");
        assert_eq!(buf[1], 0xFE, "request ID byte 1 not echoed");
        assert_eq!(buf[3] & 0x0F, 1, "expected FORMERR (RCode 1)");
    }

    // --- Shutdown, bounded ---------------------------------------------
    tx.send(()).expect("shutdown receiver dropped early");
    let shut = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("server shutdown did not complete within 5s")
        .expect("server task panicked");
    assert!(shut.is_ok(), "server task returned error: {shut:?}");
}

/// Wrap a single hickory query in a bounded timeout — CI hangs are worse
/// than CI failures.
async fn timeout<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = Result<T, ClientError>>,
{
    tokio::time::timeout(Duration::from_secs(2), fut)
        .await
        .expect("query timed out")
        .expect("query returned ClientError")
}

// Type-check `SocketAddr` import is genuinely used (silences `dead_code` if
// the integration test ever drops its explicit annotations).
const _: fn() = || {
    let _: Option<SocketAddr> = None;
};
