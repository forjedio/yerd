//! Hickory-server wiring: binds UDP+TCP, hands a [`LoopbackHandler`] to
//! `hickory_server::ServerFuture`, runs until a caller-supplied shutdown
//! future resolves.
//!
//! `Bound::bind` is two-stage from `Bound::serve` because the daemon needs the
//! resolved [`SocketAddr`] (after kernel-assigns an ephemeral port) *before*
//! starting `serve` — it has to hand the address to
//! `yerd_platform::ResolverInstaller::install`.

use std::future::Future;
use std::net::SocketAddr;

use hickory_proto::op::{Header, LowerQuery, ResponseCode};
use hickory_proto::rr::{rdata, RData, Record, RecordType};
use hickory_server::authority::MessageResponseBuilder;
use hickory_server::server::{
    Request, RequestHandler, ResponseHandler, ResponseInfo, ServerFuture,
};

use crate::answer::{Answer, QClass};
use crate::error::{BindProto, DnsError};
use crate::responder::Responder;

/// A bound UDP+TCP socket pair on a single [`SocketAddr`].
///
/// Construct via [`Bound::bind`]; consume via [`Bound::serve`].
pub struct Bound {
    udp: tokio::net::UdpSocket,
    tcp: tokio::net::TcpListener,
    local_addr: SocketAddr,
}

impl Bound {
    /// Bind UDP + TCP on the same address.
    ///
    /// `addr.ip().is_loopback()` should hold (`127.0.0.0/8` or `::1`). Binding
    /// to `0.0.0.0` / `::` would expose the responder to the LAN; this is a
    /// documented contract, **not enforced** — the daemon validates inputs
    /// before opening sockets.
    ///
    /// If `addr.port() == 0`, UDP is bound first to capture the kernel-assigned
    /// port; TCP is then bound to the same port. If TCP cannot match, UDP is
    /// dropped and the loop retries up to a fixed internal budget (5
    /// attempts). After retries are exhausted, returns
    /// [`DnsError::PortPairMismatch`]. When called with an explicit port,
    /// no retry — TCP bind failure surfaces immediately as [`DnsError::Bind`].
    ///
    /// On success, [`Bound::local_addr`] returns the actual port (which may
    /// differ from the input `addr.port()` when the latter was 0). Operator
    /// log messages should report `local_addr()`, not the input `addr`.
    pub async fn bind(addr: SocketAddr) -> Result<Self, DnsError> {
        let ephemeral = addr.port() == 0;
        let mut attempt: usize = 0;
        loop {
            attempt += 1;
            let udp = tokio::net::UdpSocket::bind(addr)
                .await
                .map_err(|source| DnsError::Bind {
                    proto: BindProto::Udp,
                    addr,
                    source,
                })?;
            let udp_addr = udp.local_addr().map_err(|source| DnsError::Bind {
                proto: BindProto::Udp,
                addr,
                source,
            })?;
            let tcp_addr = SocketAddr::new(udp_addr.ip(), udp_addr.port());
            match tokio::net::TcpListener::bind(tcp_addr).await {
                Ok(tcp) => {
                    return Ok(Self {
                        udp,
                        tcp,
                        local_addr: udp_addr,
                    });
                }
                Err(source) => {
                    if !ephemeral {
                        return Err(DnsError::Bind {
                            proto: BindProto::Tcp,
                            addr,
                            source,
                        });
                    }
                    if attempt >= crate::RETRY_BUDGET {
                        return Err(DnsError::PortPairMismatch {
                            udp_addr,
                            attempts: attempt,
                            source,
                        });
                    }
                    drop(udp);
                }
            }
        }
    }

    /// Returns the actual bound address (UDP + TCP agree by construction).
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Serve until `shutdown` resolves.
    ///
    /// On shutdown, calls hickory's `ServerFuture::shutdown_gracefully` to
    /// cooperatively drain in-flight requests; returns `Ok(())` when the
    /// drain completes.
    ///
    /// `S: Send + 'static` is required because the returned future captures
    /// `shutdown` across `.await` points; the daemon `tokio::spawn`s the
    /// returned future, which itself requires `Send + 'static`.
    pub async fn serve<S>(self, responder: Responder, shutdown: S) -> Result<(), DnsError>
    where
        S: Future<Output = ()> + Send + 'static,
    {
        let handler = LoopbackHandler { responder };
        let mut server = ServerFuture::new(handler);
        server.register_socket(self.udp);
        server.register_listener(self.tcp, std::time::Duration::from_secs(5));

        tokio::pin!(shutdown);
        tokio::select! {
            res = server.block_until_done() => res
                .map_err(|source| DnsError::ServerTask { source }),
            () = &mut shutdown => server.shutdown_gracefully().await
                .map_err(|source| DnsError::ServerTask { source }),
        }
    }
}

// `tokio::spawn(bound.serve(...))` requires the returned future to be
// `Send + 'static`. The future captures `Bound` and `Responder`; both must
// therefore be `Send + 'static` so auto-trait inference carries through. A
// future `&'a` field on either would silently break the daemon's spawn site;
// this assertion catches it at type-check time. (`const fn` with trait
// bounds: stable since Rust 1.61; empty body has no const-disallowed
// operations.)
const _: () = {
    const fn assert_send_static<T: Send + 'static>() {}
    assert_send_static::<Bound>();
    assert_send_static::<Responder>();
};

struct LoopbackHandler {
    responder: Responder,
}

#[async_trait::async_trait]
impl RequestHandler for LoopbackHandler {
    async fn handle_request<R>(&self, request: &Request, mut handle: R) -> ResponseInfo
    where
        R: ResponseHandler,
    {
        // Hickory's parser pre-handler FORMERRs malformed packets (0 or >1
        // queries) before this code runs (server_future.rs:1051-1082).
        let q: &LowerQuery = request.query();

        let qclass = match q.query_type() {
            RecordType::A => QClass::A,
            RecordType::AAAA => QClass::Aaaa,
            _ => QClass::Other,
        };

        // hickory's `Name::Display` writes a trailing dot when the name is
        // FQDN-flagged (the typical case for inbound queries). Strip it so
        // the responder's exact-match path sees the bare TLD, not
        // <bareTLD>+'.'. This trim is load-bearing.
        // (`LowerName` already lowercases; the responder also does
        // `eq_ignore_ascii_case`. Belt-and-braces.)
        let raw = q.name().to_string();
        let name = raw.trim_end_matches('.');

        let decision = self.responder.answer(name, qclass);

        // `response_from_request` copies op_code, message_type, etc. from
        // the request and sets QR=1. We only override AA and RCODE.
        let builder = MessageResponseBuilder::from_message_request(request);
        let mut header = Header::response_from_request(request.header());
        header.set_authoritative(true);

        let owner: hickory_proto::rr::Name = q.name().into();

        let answers: Vec<Record> = match decision {
            Answer::Loopback4 => vec![Record::from_rdata(
                owner,
                crate::ANSWER_TTL_SECS,
                RData::A(rdata::A(std::net::Ipv4Addr::LOCALHOST)),
            )],
            Answer::Loopback6 => vec![Record::from_rdata(
                owner,
                crate::ANSWER_TTL_SECS,
                RData::AAAA(rdata::AAAA(std::net::Ipv6Addr::LOCALHOST)),
            )],
            Answer::NoData | Answer::NxDomain => vec![],
        };
        let rcode = match decision {
            Answer::Loopback4 | Answer::Loopback6 | Answer::NoData => ResponseCode::NoError,
            Answer::NxDomain => ResponseCode::NXDomain,
        };
        header.set_response_code(rcode);

        let response = builder.build(
            header,
            answers.iter(),
            std::iter::empty::<&Record>(), // name_servers — no NS records
            std::iter::empty::<&Record>(), // soa — RFC 2308 §3: deliberately no SOA
            std::iter::empty::<&Record>(), // additionals
        );
        handle
            .send_response(response)
            .await
            .unwrap_or_else(|_| ResponseInfo::from(header))
    }
}

const _: () = {
    const fn assert<T: RequestHandler>() {}
    assert::<LoopbackHandler>();
};
