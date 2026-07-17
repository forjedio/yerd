//! Hickory-server wiring: binds UDP+TCP, hands a [`LoopbackHandler`] to
//! `hickory_server::ServerFuture`, runs until a caller-supplied shutdown
//! future resolves.
//!
//! `Bound::bind` is two-stage from `Bound::serve` because the daemon needs the
//! resolved [`SocketAddr`] (after kernel-assigns an ephemeral port) *before*
//! starting `serve` - it has to hand the address to
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
    /// `addr.ip().is_loopback()` normally holds (`127.0.0.0/8` or `::1`). In LAN
    /// mode the daemon deliberately binds `0.0.0.0` so other devices can reach
    /// the responder; the handler then applies a source-scope filter and
    /// split-horizon answers (see [`AnswerAddrs`]). The loopback expectation is
    /// a documented contract, **not enforced** here - the daemon chooses the
    /// bind address from its config before opening sockets.
    ///
    /// If `addr.port() == 0`, UDP is bound first to capture the kernel-assigned
    /// port; TCP is then bound to the same port. If TCP cannot match, UDP is
    /// dropped and the loop retries up to a fixed internal budget (5
    /// attempts). After retries are exhausted, returns
    /// [`DnsError::PortPairMismatch`]. When called with an explicit port,
    /// no retry - TCP bind failure surfaces immediately as [`DnsError::Bind`].
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
    pub async fn serve<S>(
        self,
        responder: Responder,
        answer: AnswerAddrs,
        shutdown: S,
    ) -> Result<(), DnsError>
    where
        S: Future<Output = ()> + Send + 'static,
    {
        let handler = LoopbackHandler { responder, answer };
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
// `Send + 'static`. The future captures `Bound` and `Responder`, so both must
// be `Send + 'static`; a future `&'a` field on either would silently break the
// daemon's spawn site. This assertion catches it at type-check time.
const _: () = {
    const fn assert_send_static<T: Send + 'static>() {}
    assert_send_static::<Bound>();
    assert_send_static::<Responder>();
};

/// Which addresses the responder answers `.test` queries with, and whether LAN
/// mode is active.
///
/// In LAN mode the handler applies three changes vs loopback-only: a source
/// filter ([`yerd_core::is_lan_source`]) that `Refused`s non-private queriers;
/// split-horizon (loopback-sourced queries keep resolving to `127.0.0.1` so the
/// host's own `.test` never hairpins out the NIC, while other LAN devices get
/// the routable [`Self::lan_v4`]); and AAAA `.test` answers degrade to `NoData`
/// (LAN over IPv6 is out of scope).
#[derive(Debug, Clone, Copy)]
pub struct AnswerAddrs {
    loopback_v4: std::net::Ipv4Addr,
    lan_v4: Option<std::net::Ipv4Addr>,
    lan_mode: bool,
}

impl AnswerAddrs {
    /// Loopback-only answers (LAN off): every `.test` A record is `127.0.0.1`.
    #[must_use]
    pub const fn loopback() -> Self {
        Self {
            loopback_v4: std::net::Ipv4Addr::LOCALHOST,
            lan_v4: None,
            lan_mode: false,
        }
    }

    /// LAN mode. `lan_v4 = Some(ip)` answers non-loopback LAN queriers with that
    /// routable address; `None` (discovery failed) falls back to loopback
    /// answers but keeps the source filter and AAAA-NoData behaviour.
    #[must_use]
    pub const fn lan(lan_v4: Option<std::net::Ipv4Addr>) -> Self {
        Self {
            loopback_v4: std::net::Ipv4Addr::LOCALHOST,
            lan_v4,
            lan_mode: true,
        }
    }
}

impl Default for AnswerAddrs {
    fn default() -> Self {
        Self::loopback()
    }
}

struct LoopbackHandler {
    responder: Responder,
    answer: AnswerAddrs,
}

#[async_trait::async_trait]
impl RequestHandler for LoopbackHandler {
    async fn handle_request<R>(&self, request: &Request, mut handle: R) -> ResponseInfo
    where
        R: ResponseHandler,
    {
        let q: &LowerQuery = request.query();

        let qclass = match q.query_type() {
            RecordType::A => QClass::A,
            RecordType::AAAA => QClass::Aaaa,
            _ => QClass::Other,
        };

        let raw = q.name().to_string();
        let name = raw.trim_end_matches('.');

        let src_ip = request.src().ip();
        // In LAN mode, refuse queriers outside the private/local scope before
        // answering - the DNS analogue of the proxy/bootstrap peer filter.
        let decision = if self.answer.lan_mode && !yerd_core::is_lan_source(src_ip) {
            Answer::Refused
        } else {
            self.responder.answer(name, qclass)
        };

        let builder = MessageResponseBuilder::from_message_request(request);
        let mut header = Header::response_from_request(request.header());
        header.set_authoritative(!matches!(decision, Answer::Refused));

        let owner: hickory_proto::rr::Name = q.name().into();

        let answers: Vec<Record> = match decision {
            Answer::Loopback4 => {
                // Split-horizon: loopback-sourced queries (incl. the host's own)
                // keep resolving to loopback; other LAN devices get the routable
                // LAN IPv4 (falling back to loopback if discovery yielded none).
                let ip = if self.answer.lan_mode && !src_ip.is_loopback() {
                    self.answer.lan_v4.unwrap_or(self.answer.loopback_v4)
                } else {
                    self.answer.loopback_v4
                };
                vec![Record::from_rdata(
                    owner,
                    crate::ANSWER_TTL_SECS,
                    RData::A(rdata::A(ip)),
                )]
            }
            // In LAN mode AAAA `.test` degrades to NoData (empty, NoError) - LAN
            // over IPv6/link-local is out of scope.
            Answer::Loopback6 if self.answer.lan_mode => vec![],
            Answer::Loopback6 => vec![Record::from_rdata(
                owner,
                crate::ANSWER_TTL_SECS,
                RData::AAAA(rdata::AAAA(std::net::Ipv6Addr::LOCALHOST)),
            )],
            Answer::NoData | Answer::NxDomain | Answer::Refused => vec![],
        };
        let rcode = match decision {
            Answer::Loopback4 | Answer::Loopback6 | Answer::NoData => ResponseCode::NoError,
            Answer::NxDomain => ResponseCode::NXDomain,
            Answer::Refused => ResponseCode::Refused,
        };
        header.set_response_code(rcode);

        let response = builder.build(
            header,
            answers.iter(),
            std::iter::empty::<&Record>(), // name_servers
            std::iter::empty::<&Record>(), // soa: RFC 2308 §3, no SOA
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    //! Socket-free unit coverage of `LoopbackHandler::handle_request`: build a
    //! `Request` from an in-memory query packet, drive the handler with a
    //! capturing `ResponseHandler`, and decode the emitted bytes back into a
    //! `Message` to assert the wire shape (RCODE, AA bit, answer records). No
    //! UDP/TCP socket is bound and no live resolver is involved.

    use std::net::SocketAddr;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    use hickory_proto::op::{Message, MessageType, OpCode, Query};
    use hickory_proto::rr::Name;
    use hickory_proto::serialize::binary::{BinDecodable, BinDecoder, BinEncodable, BinEncoder};
    use hickory_server::authority::{MessageRequest, MessageResponse};
    use hickory_server::server::Protocol;

    use yerd_core::Tld;

    use super::*;

    /// A `ResponseHandler` that serialises the handler's response into a shared
    /// buffer (mirrors hickory's own `ResponseHandle::send_response` encode
    /// path) so the test can decode and inspect it.
    #[derive(Clone)]
    struct CaptureHandler {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    #[async_trait::async_trait]
    impl ResponseHandler for CaptureHandler {
        async fn send_response<'a>(
            &mut self,
            response: MessageResponse<
                '_,
                'a,
                impl Iterator<Item = &'a Record> + Send + 'a,
                impl Iterator<Item = &'a Record> + Send + 'a,
                impl Iterator<Item = &'a Record> + Send + 'a,
                impl Iterator<Item = &'a Record> + Send + 'a,
            >,
        ) -> std::io::Result<ResponseInfo> {
            let mut buffer = Vec::with_capacity(512);
            let info = {
                let mut encoder = BinEncoder::new(&mut buffer);
                response
                    .destructive_emit(&mut encoder)
                    .map_err(|e| std::io::Error::other(format!("{e}")))?
            };
            *self.buf.lock().unwrap() = buffer;
            Ok(info)
        }
    }

    /// Round-trip through the wire so we get a genuine `MessageRequest`,
    /// exactly as hickory's `handle_raw_request` would after a socket read.
    fn build_request_from(qname: &str, qtype: RecordType, src: &str) -> Request {
        let name = Name::from_str(qname).unwrap();
        let query = Query::query(name, qtype);
        let mut msg = Message::new();
        msg.set_id(0x1234)
            .set_message_type(MessageType::Query)
            .set_op_code(OpCode::Query)
            .set_recursion_desired(true)
            .add_query(query);
        let bytes = msg.to_bytes().unwrap();
        let mut decoder = BinDecoder::new(&bytes);
        let req = MessageRequest::read(&mut decoder).unwrap();
        let src: SocketAddr = src.parse().unwrap();
        Request::new(req, src, Protocol::Udp)
    }

    async fn handle(tld: &str, qname: &str, qtype: RecordType) -> Message {
        handle_with(tld, qname, qtype, AnswerAddrs::loopback(), "127.0.0.1:5353").await
    }

    async fn handle_with(
        tld: &str,
        qname: &str,
        qtype: RecordType,
        answer: AnswerAddrs,
        src: &str,
    ) -> Message {
        let handler = LoopbackHandler {
            responder: Responder::new(Tld::new(tld).unwrap()),
            answer,
        };
        let request = build_request_from(qname, qtype, src);
        let buf = Arc::new(Mutex::new(Vec::new()));
        let capture = CaptureHandler {
            buf: Arc::clone(&buf),
        };
        let _info = handler.handle_request(&request, capture).await;
        let bytes = buf.lock().unwrap().clone();
        assert!(!bytes.is_empty(), "handler emitted no response bytes");
        let resp = Message::from_bytes(&bytes).unwrap();
        assert_eq!(resp.id(), 0x1234, "response must echo request id");
        assert_eq!(resp.message_type(), MessageType::Response);
        resp
    }

    #[tokio::test]
    async fn a_query_in_zone_yields_authoritative_loopback4() {
        let resp = handle("test", "app.test.", RecordType::A).await;
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert!(resp.header().authoritative(), "in-zone reply must set AA");
        assert_eq!(resp.answers().len(), 1);
        let rec = &resp.answers()[0];
        assert_eq!(rec.ttl(), crate::ANSWER_TTL_SECS);
        match rec.data() {
            Some(RData::A(rdata::A(ip))) => assert_eq!(*ip, std::net::Ipv4Addr::LOCALHOST),
            other => panic!("expected RData::A, got {other:?}"),
        }
    }

    fn only_a(resp: &Message) -> std::net::Ipv4Addr {
        assert_eq!(resp.answers().len(), 1);
        match resp.answers()[0].data() {
            Some(RData::A(rdata::A(ip))) => *ip,
            other => panic!("expected RData::A, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn lan_mode_non_loopback_source_gets_lan_ip() {
        let lan = std::net::Ipv4Addr::new(192, 168, 1, 42);
        let resp = handle_with(
            "test",
            "app.test.",
            RecordType::A,
            AnswerAddrs::lan(Some(lan)),
            "192.168.1.9:5353",
        )
        .await;
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert_eq!(only_a(&resp), lan);
    }

    #[tokio::test]
    async fn lan_mode_loopback_source_still_gets_loopback() {
        let lan = std::net::Ipv4Addr::new(192, 168, 1, 42);
        let resp = handle_with(
            "test",
            "app.test.",
            RecordType::A,
            AnswerAddrs::lan(Some(lan)),
            "127.0.0.1:5353",
        )
        .await;
        assert_eq!(only_a(&resp), std::net::Ipv4Addr::LOCALHOST);
    }

    #[tokio::test]
    async fn lan_mode_without_discovered_ip_falls_back_to_loopback() {
        let resp = handle_with(
            "test",
            "app.test.",
            RecordType::A,
            AnswerAddrs::lan(None),
            "192.168.1.9:5353",
        )
        .await;
        assert_eq!(only_a(&resp), std::net::Ipv4Addr::LOCALHOST);
    }

    #[tokio::test]
    async fn lan_mode_aaaa_degrades_to_nodata() {
        let lan = std::net::Ipv4Addr::new(192, 168, 1, 42);
        let resp = handle_with(
            "test",
            "app.test.",
            RecordType::AAAA,
            AnswerAddrs::lan(Some(lan)),
            "192.168.1.9:5353",
        )
        .await;
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert!(resp.header().authoritative());
        assert_eq!(resp.answers().len(), 0, "AAAA must be NoData in LAN mode");
    }

    #[tokio::test]
    async fn lan_mode_refuses_non_private_source() {
        let lan = std::net::Ipv4Addr::new(192, 168, 1, 42);
        let resp = handle_with(
            "test",
            "app.test.",
            RecordType::A,
            AnswerAddrs::lan(Some(lan)),
            "8.8.8.8:5353",
        )
        .await;
        assert_eq!(resp.response_code(), ResponseCode::Refused);
        assert_eq!(resp.answers().len(), 0);
    }

    #[tokio::test]
    async fn aaaa_query_in_zone_yields_loopback6() {
        let resp = handle("test", "app.test.", RecordType::AAAA).await;
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert!(resp.header().authoritative());
        assert_eq!(resp.answers().len(), 1);
        match resp.answers()[0].data() {
            Some(RData::AAAA(rdata::AAAA(ip))) => assert_eq!(*ip, std::net::Ipv6Addr::LOCALHOST),
            other => panic!("expected RData::AAAA, got {other:?}"),
        }
    }

    /// MX yields `NoData`: NOERROR, empty answer, AA still set (it is our zone).
    #[tokio::test]
    async fn non_address_qtype_in_zone_yields_nodata() {
        let resp = handle("test", "app.test.", RecordType::MX).await;
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert!(resp.header().authoritative());
        assert_eq!(resp.answers().len(), 0);
        assert_eq!(resp.name_servers().len(), 0, "RFC 2308 §3: no SOA");
    }

    #[tokio::test]
    async fn apex_a_query_yields_nodata() {
        let resp = handle("test", "test.", RecordType::A).await;
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert!(resp.header().authoritative());
        assert_eq!(resp.answers().len(), 0);
    }

    #[tokio::test]
    async fn out_of_zone_query_yields_refused_non_authoritative() {
        let resp = handle("test", "unrelated.com.", RecordType::A).await;
        assert_eq!(resp.response_code(), ResponseCode::Refused);
        assert!(
            !resp.header().authoritative(),
            "out-of-zone reply must clear the AA bit"
        );
        assert_eq!(resp.answers().len(), 0);
    }

    #[tokio::test]
    async fn multi_label_tld_in_zone_yields_loopback4() {
        let resp = handle("dev.local", "app.dev.local.", RecordType::A).await;
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert!(resp.header().authoritative());
        assert_eq!(resp.answers().len(), 1);
    }
}
