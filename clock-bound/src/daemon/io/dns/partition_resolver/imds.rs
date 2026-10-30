//! Minimal `IMDSv2` client for `/latest/meta-data/services/partition`.
//!
//! `IMDSv2` only: a session token is fetched with `PUT /latest/api/token` and sent on
//! the metadata `GET`. There is no `IMDSv1` fallback.
//!
//! IMDS is the only way to detect which partition we are running in, and decide which
//! public Amazon Time Sync service domain to use. This is only intended to be called
//! when an Amazon EC2 instance (Nitro or Xen) is detected.

use std::fmt;
use std::io::Write as _;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::debug;

use super::Partition;

/// IMDS endpoints, tried in order: IPv4 first, then IPv6.
const ENDPOINTS: [SocketAddr; 2] = [
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(169, 254, 169, 254), 80)),
    SocketAddr::V6(SocketAddrV6::new(
        Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254),
        80,
        0,
        0,
    )),
];

/// Timeout for a single HTTP request (connect, write, and read).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

const TOKEN_PATH: &str = "/latest/api/token";
const PARTITION_PATH: &str = "/latest/meta-data/services/partition";
const TOKEN_TTL_HEADER: &str = "X-aws-ec2-metadata-token-ttl-seconds";
/// The token is used immediately, so a short TTL is enough.
const TOKEN_TTL_SECS: &str = "60";
const TOKEN_HEADER: &str = "X-aws-ec2-metadata-token";
/// Maximum accepted response body size.
const MAX_BODY_BYTES: usize = 4096;
/// Maximum accepted size of the whole response (status line, headers, and body).
///
/// This is also the size of the receive buffer.
const MAX_RESPONSE_BYTES: usize = 8192;
/// Capacity reserved for the send buffer.
///
/// The largest request is the partition `GET`, whose only variable part is the token
/// (at most [`MAX_BODY_BYTES`]). The fixed part is well under 512 bytes, so the send
/// buffer never reallocates (which would leave an un-wiped copy of the old contents).
const SEND_BUF_CAPACITY: usize = MAX_BODY_BYTES + 512;

/// Errors from a single IMDS request.
#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
    #[error("IMDS connection IO failed")]
    Io(#[from] std::io::Error),
    #[error("IMDS request timed out")]
    Timeout,
    #[error("IMDS returned HTTP status {0}")]
    Status(u16),
    #[error("IMDS returned a malformed HTTP response")]
    Malformed,
    #[error("IMDS response exceeded the size limit")]
    ResponseTooLarge,
}

/// `IMDSv2` client for the instance's AWS partition.
///
/// The two buffers are separate fields so a body borrowed from `recv` (the session
/// token) can be written into `send` without copying it out first.
pub(super) struct Imds {
    /// The request for the next [`Imds::exchange`].
    send: RequestBuf,
    /// The response of the last [`Imds::exchange`].
    recv: ResponseBuf,
}

/// Doesn't print the buffers, which may hold a session token.
impl fmt::Debug for Imds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Imds").finish_non_exhaustive()
    }
}

#[cfg_attr(test, mockall::automock)]
#[cfg_attr(
    test,
    expect(
        dead_code,
        reason = "tests of the parent module use `MockImds` instead"
    )
)]
impl Imds {
    /// Fetch the AWS [`Partition`], trying each IMDS endpoint in order (IPv4, then IPv6).
    ///
    /// The first success wins. Returns the last endpoint's error if all fail.
    pub(super) async fn fetch_partition(&mut self) -> Result<Partition, Error> {
        self.fetch_partition_from(&ENDPOINTS).await
    }
}

// Kept out of the automocked block: `MockImds` provides its own `new`, and the
// remaining helpers are generic or return borrowed data, which `automock` can't mock.
impl Imds {
    /// Constructs the client, allocating its buffers. No IO is performed.
    pub(super) fn new() -> Self {
        Self {
            send: RequestBuf::new(),
            recv: ResponseBuf::new(),
        }
    }

    /// Try each of `endpoints` in order; the first success wins.
    ///
    /// Returns the last endpoint's error if all fail. `endpoints` must not be empty.
    async fn fetch_partition_from(&mut self, endpoints: &[SocketAddr]) -> Result<Partition, Error> {
        let mut last_error = None;
        for &endpoint in endpoints {
            match self.fetch_partition_at(endpoint).await {
                Ok(partition) => return Ok(partition),
                Err(error) => {
                    debug!(%endpoint, ?error, "IMDS endpoint failed.");
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.expect("IMDS endpoint list is never empty"))
    }

    /// Fetch the AWS [`Partition`] from the IMDS at `endpoint`.
    ///
    /// Retrieves an `IMDSv2` session token (`PUT /latest/api/token`), then requests the
    /// partition (`GET /latest/meta-data/services/partition`) with it. Logs the raw
    /// partition string reported by IMDS (e.g. `aws`, `aws-cn`, `aws-us-gov`).
    async fn fetch_partition_at(&mut self, endpoint: SocketAddr) -> Result<Partition, Error> {
        self.send.build(
            Method::Put,
            endpoint,
            TOKEN_PATH,
            (TOKEN_TTL_HEADER, TOKEN_TTL_SECS),
        );
        // Borrowed from `self.recv`, which stays untouched while `self.send` is rebuilt.
        let token = parse_token(
            Self::connect_and_exchange(&mut self.send, &mut self.recv, endpoint).await?,
        )?;

        // The token is copied into the send buffer her
        self.send
            .build(Method::Get, endpoint, PARTITION_PATH, (TOKEN_HEADER, token));
        let raw_partition = Self::connect_and_exchange(&mut self.send, &mut self.recv, endpoint)
            .await?
            .trim();
        debug!(%endpoint, %raw_partition, "IMDS reported AWS partition.");
        Ok(Partition::from_imds(raw_partition))
    }

    /// Connect to `endpoint` and [`exchange`](Imds::exchange) the request in `send`, all
    /// bounded by [`REQUEST_TIMEOUT`]. Returns the untrimmed `200` body, borrowed from
    /// `recv`.
    async fn connect_and_exchange<'r>(
        send: &mut RequestBuf,
        recv: &'r mut ResponseBuf,
        endpoint: SocketAddr,
    ) -> Result<&'r str, Error> {
        tokio::time::timeout(REQUEST_TIMEOUT, async move {
            let mut stream = TcpStream::connect(endpoint).await?;
            Self::exchange(send, recv, &mut stream).await
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    /// Send the request in `send` over `stream`, then read until a complete response is
    /// parsed. Returns the untrimmed `200` body, borrowed from `recv` only, so `send` can
    /// be rebuilt while the body is still in use.
    ///
    /// Zeroization order: `recv` is wiped first, erasing the previous response. `send`
    /// holds the current request (built by [`RequestBuf::build`], which zeroes it before
    /// writing), so it is wiped right after the write instead, whether or not the write
    /// succeeded.
    async fn exchange<'r, S>(
        send: &mut RequestBuf,
        recv: &'r mut ResponseBuf,
        stream: &mut S,
    ) -> Result<&'r str, Error>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        recv.wipe();

        let written = stream.write_all(send.as_bytes()).await;
        send.wipe();
        written?;

        recv.read_from(stream).await
    }
}

/// Send buffer holding one HTTP request, built by [`RequestBuf::build`].
///
/// Allocated once with [`SEND_BUF_CAPACITY`] and never reallocated, so no un-wiped
/// copy of an old request is left behind.
struct RequestBuf {
    buf: Vec<u8>,
}

/// Doesn't print the request, which may hold a session token.
impl fmt::Debug for RequestBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBuf").finish_non_exhaustive()
    }
}

impl RequestBuf {
    /// Allocates an empty buffer with [`SEND_BUF_CAPACITY`].
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(SEND_BUF_CAPACITY),
        }
    }

    /// Build an HTTP/1.1 request with a single extra header, replacing (and first
    /// zeroing) the previous contents.
    fn build(
        &mut self,
        method: Method,
        endpoint: SocketAddr,
        path: &str,
        (name, value): (&str, &str),
    ) {
        self.wipe();
        let content_length = match method {
            Method::Put => "Content-Length: 0\r\n",
            Method::Get => "",
        };
        write!(
            self.buf,
            "{method} {path} HTTP/1.1\r\nHost: {endpoint}\r\n{name}: {value}\r\n{content_length}Connection: close\r\n\r\n",
            method = method.as_str(),
        )
        .expect("writing to a Vec cannot fail");
        // A larger request would have reallocated the send buffer. Guard against that.
        //
        // This module does not publicly expose anything that could arbitrarily grow this.
        assert!(
            self.buf.len() <= SEND_BUF_CAPACITY,
            "IMDS request of {} bytes exceeds the send buffer capacity of {SEND_BUF_CAPACITY}",
            self.buf.len(),
        );
    }

    /// The current request.
    fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Zero the request, then clear it. The capacity is kept.
    fn wipe(&mut self) {
        self.buf.fill(0);
        self.buf.clear();
    }
}

/// Receive buffer of exactly [`MAX_RESPONSE_BYTES`]; reads go straight into it.
struct ResponseBuf {
    buf: Box<[u8]>,
}

/// Doesn't print the response, which may hold a session token.
impl fmt::Debug for ResponseBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseBuf").finish_non_exhaustive()
    }
}

impl ResponseBuf {
    /// Allocates a zeroed buffer of [`MAX_RESPONSE_BYTES`].
    fn new() -> Self {
        Self {
            buf: vec![0; MAX_RESPONSE_BYTES].into_boxed_slice(),
        }
    }

    /// Read from `stream` into the start of the buffer until a complete response is
    /// parsed. Returns the untrimmed `200` body, borrowed from the buffer.
    ///
    /// Doesn't wipe first; the caller does (see [`Imds::exchange`]).
    async fn read_from<S>(&mut self, stream: &mut S) -> Result<&str, Error>
    where
        S: AsyncRead + Unpin,
    {
        let mut filled = 0;
        let eof = loop {
            // Read straight into the unused tail of the receive buffer. `read` returns
            // `Ok(0)` at EOF.
            let n = stream.read(&mut self.buf[filled..]).await?;
            filled += n;
            let eof = n == 0;
            if eof || parse_response(&self.buf[..filled], false)?.is_some() {
                break eof;
            }
            // Buffer full and the response still incomplete.
            if filled == self.buf.len() {
                return Err(Error::ResponseTooLarge);
            }
        };
        // Parse once more to return the body: returning it from inside the loop would
        // keep the receive buffer borrowed across the next iteration's read. With `eof`
        // set, `parse_response` never returns `Ok(None)`.
        parse_response(&self.buf[..filled], eof)?.ok_or(Error::Malformed)
    }

    /// Zero the whole buffer.
    fn wipe(&mut self) {
        self.buf.fill(0);
    }
}

/// HTTP request method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Get,
    Put,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Put => "PUT",
        }
    }
}

/// Validate the body of a token response, returning the trimmed token borrowed from
/// `body`.
///
/// Rejects tokens that are empty or contain anything other than printable ASCII, since
/// the token is echoed into a request header.
fn parse_token(body: &str) -> Result<&str, Error> {
    let token = body.trim();
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Error::Malformed);
    }
    Ok(token)
}

/// Parse a raw HTTP/1.x response that may have only partly arrived.
///
/// This is called after every read with all the bytes received so far. It's a
/// deliberately minimal parser: IMDS sends small, simple responses, so anything
/// unusual is rejected rather than handled.
///
/// # Layout
///
/// ```text
/// HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\naws
/// └─status line─┘    └──── header ───┘└─sep──┘└─┘
/// └────────────── head ──────────────┘▲       body
///                                     header_end
/// ```
///
/// - `header_end` is the index of the first `\r\n\r\n`.
/// - The head (`raw[..header_end]`) is split on `\r\n`. The first line is the status
///   line and the rest are headers. Only `Content-Length` is used.
/// - The body is `raw[header_end + 4..]`. With `Content-Length`, exactly that many
///   bytes are taken and any bytes after them are ignored. Without it, the body runs
///   until EOF.
///
/// # Returns
///
/// - `Ok(Some(body))`: a complete `200` response. `body` is borrowed from `raw` and
///   is not trimmed.
/// - `Ok(None)`: more bytes are needed. Never returned when `eof` is `true`.
///
/// # Errors
///
/// - [`Error::Status`] if the status code is anything other than `200`. This is
///   returned as soon as the head arrives, without waiting for the body.
/// - [`Error::ResponseTooLarge`] if `Content-Length` is over [`MAX_BODY_BYTES`], or if
///   there is no `Content-Length` and the body is over [`MAX_BODY_BYTES`] at EOF.
/// - [`Error::Malformed`] if:
///   - `eof` is set before the head's closing `\r\n\r\n` arrives
///   - the head is not valid UTF-8
///   - the status line is not `HTTP/1.x <3 digits> [reason]`
///   - a header line has no `:`
///   - a `Transfer-Encoding` header is present (chunked framing isn't supported)
///   - `Content-Length` appears more than once or isn't a non-negative integer
///   - `eof` is set before `Content-Length` bytes of body have arrived
///   - the body is not valid UTF-8
///
/// # Examples
///
/// A complete response:
///
/// ```text
/// Raw bytes:
/// HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\naws
///
/// On the wire:
/// HTTP/1.1 200 OK
/// Content-Length: 3
///
/// aws
///
/// parse_response(raw, false) == Ok(Some("aws"))
/// ```
///
/// Header names are matched without regard to case:
///
/// ```text
/// Raw bytes:
/// HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\naws-cn
///
/// On the wire:
/// HTTP/1.1 200 OK
/// content-length: 6
///
/// aws-cn
///
/// parse_response(raw, false) == Ok(Some("aws-cn"))
/// ```
///
/// An incomplete response, where either the head or the body is still arriving:
///
/// ```text
/// Raw bytes (head not finished):
/// HTTP/1.1 200 OK\r\nContent-Len
///
/// On the wire:
/// HTTP/1.1 200 OK
/// Content-Len
///
/// Raw bytes (body has 3 of 6 bytes):
/// HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\naws
///
/// On the wire:
/// HTTP/1.1 200 OK
/// Content-Length: 6
///
/// aws
///
/// For both:
/// parse_response(raw, false) == Ok(None)              // keep reading
/// parse_response(raw, true)  == Err(Error::Malformed) // server closed early
/// ```
fn parse_response(raw: &[u8], eof: bool) -> Result<Option<&str>, Error> {
    // An HTTP response is `<status line>\r\n<headers>\r\n\r\n<body>`. Find the blank line
    // (`\r\n\r\n`) that ends the status line + headers ("head") and starts the body.
    let Some(header_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        // Head not fully received yet. If the server already closed the connection it
        // never will be, so the response is broken; otherwise wait for more bytes.
        return if eof { Err(Error::Malformed) } else { Ok(None) };
    };
    // The head is ASCII text in practice; refuse anything that isn't valid UTF-8 so the
    // line/header parsing below can work on `&str`.
    let head = std::str::from_utf8(&raw[..header_end]).map_err(|_| Error::Malformed)?;
    // Everything after the 4-byte `\r\n\r\n` separator is (possibly partial) body.
    let body = &raw[header_end + 4..];

    // Head lines are separated by CRLF. The first line is the status line, the rest are
    // headers.
    let mut lines = head.split("\r\n");
    // `split` always yields at least one item, so `unwrap_or_default` never fires in
    // practice; it just avoids a panic path. An empty line fails status-line parsing.
    let status = parse_status_line(lines.next().unwrap_or_default())?;
    // Only `200 OK` carries a usable answer. Fail fast on anything else (e.g. `401` for
    // a bad token, `404` if the metadata path doesn't exist) without waiting for a body.
    if status != 200 {
        return Err(Error::Status(status));
    }
    // The remaining lines are headers; we only care about `Content-Length`, which tells
    // us how many body bytes to expect.
    let content_length = parse_content_length(lines)?;

    // Decide whether the body is complete, still arriving, or invalid.
    let body = match content_length {
        // Server announced a body larger than we're willing to accept: bail out before
        // buffering it.
        Some(len) if len > MAX_BODY_BYTES => return Err(Error::ResponseTooLarge),
        // All announced bytes have arrived. Take exactly `len` bytes and ignore anything
        // after them (we never reuse the connection, so trailing bytes don't matter).
        Some(len) if body.len() >= len => &body[..len],
        // Fewer bytes than announced, and the connection is closed: truncated response.
        Some(_) if eof => return Err(Error::Malformed),
        // Fewer bytes than announced, connection still open: wait for more.
        Some(_) => return Ok(None),
        // No `Content-Length`: per HTTP/1.x the body runs until the server closes the
        // connection, so keep reading until EOF.
        None if !eof => return Ok(None),
        // No `Content-Length` and EOF reached: the whole remainder is the body, but
        // still enforce the size limit.
        None if body.len() > MAX_BODY_BYTES => return Err(Error::ResponseTooLarge),
        // No `Content-Length`, EOF reached, within the limit: the remainder is the body.
        None => body,
    };
    // Callers treat the body as text (a token or a partition name), so require UTF-8.
    // Wrap in `Some` to signal "complete response".
    std::str::from_utf8(body)
        .map(Some)
        .map_err(|_| Error::Malformed)
}

/// Parse `HTTP/1.x <3-digit code> [reason]`, returning the status code.
fn parse_status_line(line: &str) -> Result<u16, Error> {
    // Split into at most 3 parts: version, code, and the reason phrase. The limit keeps
    // a multi-word reason (e.g. `Not Found`) together as the unused third part.
    let mut parts = line.splitn(3, ' ');
    // Version and code are required; the reason phrase is optional and ignored.
    let (Some(version), Some(code)) = (parts.next(), parts.next()) else {
        return Err(Error::Malformed);
    };
    // Accept only HTTP/1.0 and HTTP/1.1 (what IMDS speaks), and require the code to be
    // exactly three ASCII digits. The explicit digit check rejects inputs that `u16`
    // parsing would otherwise accept, such as `+20`.
    if !version.starts_with("HTTP/1.")
        || code.len() != 3
        || !code.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error::Malformed);
    }
    // Three ASCII digits always fit in a `u16`, so this cannot fail after the check
    // above; mapping the error keeps the function panic-free.
    code.parse().map_err(|_| Error::Malformed)
}

/// Scan header lines for `Content-Length`. `Transfer-Encoding` is rejected.
fn parse_content_length<'a>(lines: impl Iterator<Item = &'a str>) -> Result<Option<usize>, Error> {
    // `None` until a `Content-Length` header is seen.
    let mut content_length = None;
    for line in lines {
        // Every header must be `Name: value`. A line without a colon isn't a valid
        // header, so the whole response is rejected.
        let (name, value) = line.split_once(':').ok_or(Error::Malformed)?;
        // Tolerate whitespace around the header name.
        let name = name.trim();
        // `Transfer-Encoding` (e.g. `chunked`) changes how the body is framed, which this
        // parser doesn't implement. Reject it rather than misread the body. IMDS doesn't
        // use it for these small responses.
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(Error::Malformed);
        }
        // Header names are case-insensitive. Skip every header other than
        // `Content-Length`.
        if !name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        // A second `Content-Length` is ambiguous (which one frames the body?), so treat
        // it as malformed instead of picking one.
        if content_length.is_some() {
            return Err(Error::Malformed);
        }
        // The value must be a non-negative integer, ignoring surrounding whitespace.
        content_length = Some(value.trim().parse().map_err(|_| Error::Malformed)?);
    }
    // `None` means no `Content-Length` header, so the body is delimited by EOF.
    Ok(content_length)
}

#[cfg(test)]
mod tests {
    use super::*;

    use wiremock::matchers::{any, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // The TCP connect path and endpoint fallback are tested against a `wiremock` IMDS,
    // whose mocks enforce the `IMDSv2` request shape and are verified when the server
    // drops. HTTP framing is tested against [`Imds::exchange`] over an in-memory duplex
    // stream.

    /// Mount the token `PUT`, answered with `200` and `body`. Expected exactly once.
    async fn mount_token(server: &MockServer, body: &str) {
        Mock::given(method("PUT"))
            .and(path(TOKEN_PATH))
            .and(header(TOKEN_TTL_HEADER, TOKEN_TTL_SECS))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(server)
            .await;
    }

    /// Mount the partition `GET` carrying `token`, answered with `response`. Expected
    /// exactly once.
    ///
    /// A lower-priority catch-all answers a partition `GET` without that token with
    /// `401`, as IMDS does, and is expected never to be hit.
    async fn mount_partition(server: &MockServer, token: &str, response: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(PARTITION_PATH))
            .and(header(TOKEN_HEADER, token))
            .respond_with(response)
            .expect(1)
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(PARTITION_PATH))
            .respond_with(ResponseTemplate::new(401))
            .with_priority(u8::MAX)
            .expect(0)
            .named("partition GET without the session token")
            .mount(server)
            .await;
    }

    /// Mount a healthy IMDS: a token, then `partition` for a `GET` carrying it.
    async fn mount_imds(server: &MockServer, token: &str, partition: &str) {
        mount_token(server, token).await;
        mount_partition(
            server,
            token.trim(),
            ResponseTemplate::new(200).set_body_string(partition),
        )
        .await;
    }

    /// Mount a catch-all that must never be hit: no request at all reaches `server`
    /// beyond those matched by higher-priority mocks.
    async fn mount_no_other_requests(server: &MockServer) {
        Mock::given(any())
            .respond_with(ResponseTemplate::new(500))
            .with_priority(u8::MAX)
            .expect(0)
            .named("unexpected request")
            .mount(server)
            .await;
    }

    /// Read a request until the end of its headers (requests carry no body).
    async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> String {
        let mut raw = Vec::new();
        let mut buf = [0u8; 64];
        while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream
                .read(&mut buf)
                .await
                .unwrap_or_else(|e| panic!("fake IMDS read failed: {e}"));
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
        }
        String::from_utf8(raw).unwrap()
    }

    /// A response with `Content-Length` set from `body`.
    fn http_response(status_line: &str, body: &str) -> Vec<u8> {
        format!(
            "{status_line}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    /// An address nothing listens on, so connecting is refused.
    fn refused_addr() -> SocketAddr {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap()
    }

    /// Run one [`Imds::exchange`] over a duplex stream whose internal buffer holds at
    /// most `max_buf_size` bytes, so a small size forces the request and response to be
    /// split across many reads and writes.
    ///
    /// The peer reads the request, writes `response`, then closes (EOF). Returns the
    /// owned body and the request the peer received.
    async fn duplex_exchange(
        imds: &mut Imds,
        response: &[u8],
        max_buf_size: usize,
    ) -> (Result<String, Error>, String) {
        let (mut client, mut server) = tokio::io::duplex(max_buf_size);
        let client_side = async move {
            let result = Imds::exchange(&mut imds.send, &mut imds.recv, &mut client)
                .await
                .map(str::to_owned);
            // Hang up so a peer blocked writing an unread response gets an error.
            drop(client);
            result
        };
        let server_side = async move {
            let request = read_request(&mut server).await;
            // The client may already have hung up; that is not a test failure.
            let _ = server.write_all(response).await;
            drop(server);
            request
        };
        tokio::join!(client_side, server_side)
    }

    /// An [`Imds`] with the partition `GET` for `ENDPOINTS[0]` in its send buffer.
    fn imds_with_get_request() -> Imds {
        let mut imds = Imds::new();
        imds.send.build(
            Method::Get,
            ENDPOINTS[0],
            PARTITION_PATH,
            (TOKEN_HEADER, "token123"),
        );
        imds
    }

    const OK: &str = "HTTP/1.1 200 OK";

    #[test]
    fn parse_response_with_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\naws";
        assert_eq!(parse_response(raw, false).unwrap(), Some("aws"));
    }

    #[test]
    fn parse_response_content_length_is_case_insensitive() {
        let raw = b"HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\naws-cn";
        assert_eq!(parse_response(raw, false).unwrap(), Some("aws-cn"));
    }

    #[test]
    fn parse_response_ignores_bytes_past_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nawsXYZ";
        assert_eq!(parse_response(raw, false).unwrap(), Some("aws"));
    }

    #[test]
    fn parse_response_incomplete_headers_needs_more() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Len";
        assert_eq!(parse_response(raw, false).unwrap(), None);
    }

    #[test]
    fn parse_response_incomplete_headers_at_eof_is_malformed() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Len";
        assert!(matches!(parse_response(raw, true), Err(Error::Malformed)));
    }

    #[test]
    fn parse_response_partial_body_needs_more() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\naws";
        assert_eq!(parse_response(raw, false).unwrap(), None);
    }

    #[test]
    fn parse_response_partial_body_at_eof_is_malformed() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\naws";
        assert!(matches!(parse_response(raw, true), Err(Error::Malformed)));
    }

    #[test]
    fn parse_response_without_content_length_reads_to_eof() {
        let raw = b"HTTP/1.1 200 OK\r\nServer: EC2ws\r\n\r\naws";
        assert_eq!(parse_response(raw, false).unwrap(), None);
        assert_eq!(parse_response(raw, true).unwrap(), Some("aws"));
    }

    #[test]
    fn parse_response_non_200_is_status() {
        let raw = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n";
        assert!(matches!(
            parse_response(raw, false),
            Err(Error::Status(401))
        ));
    }

    #[test]
    fn parse_response_garbage_status_line_is_malformed() {
        assert!(matches!(
            parse_response(b"garbage\r\n\r\n", false),
            Err(Error::Malformed)
        ));
    }

    #[test]
    fn parse_response_non_numeric_status_is_malformed() {
        assert!(matches!(
            parse_response(b"HTTP/1.1 abc OK\r\n\r\n", false),
            Err(Error::Malformed)
        ));
    }

    #[test]
    fn parse_response_header_without_colon_is_malformed() {
        assert!(matches!(
            parse_response(b"HTTP/1.1 200 OK\r\nbogus\r\n\r\n", false),
            Err(Error::Malformed)
        ));
    }

    #[test]
    fn parse_response_chunked_is_malformed() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\naws\r\n0\r\n\r\n";
        assert!(matches!(parse_response(raw, false), Err(Error::Malformed)));
    }

    #[test]
    fn parse_response_duplicate_content_length_is_malformed() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 3\r\n\r\naws";
        assert!(matches!(parse_response(raw, false), Err(Error::Malformed)));
    }

    #[test]
    fn parse_response_large_content_length_is_too_large() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5000\r\n\r\n";
        assert!(matches!(
            parse_response(raw, false),
            Err(Error::ResponseTooLarge)
        ));
    }

    #[test]
    fn parse_response_large_body_without_content_length_is_too_large() {
        let mut raw = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        raw.extend(std::iter::repeat_n(b'a', 5000));
        assert!(matches!(
            parse_response(&raw, true),
            Err(Error::ResponseTooLarge)
        ));
    }

    #[test]
    fn parse_response_non_utf8_body_is_malformed() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n\xff\xfe";
        assert!(matches!(parse_response(raw, false), Err(Error::Malformed)));
    }

    #[test]
    fn build_put_request() {
        let mut send = RequestBuf::new();
        send.build(
            Method::Put,
            ENDPOINTS[0],
            TOKEN_PATH,
            (TOKEN_TTL_HEADER, TOKEN_TTL_SECS),
        );
        assert_eq!(
            std::str::from_utf8(send.as_bytes()).unwrap(),
            "PUT /latest/api/token HTTP/1.1\r\n\
             Host: 169.254.169.254:80\r\n\
             X-aws-ec2-metadata-token-ttl-seconds: 60\r\n\
             Content-Length: 0\r\n\
             Connection: close\r\n\r\n"
        );
    }

    #[test]
    fn build_get_request() {
        let mut send = RequestBuf::new();
        send.build(
            Method::Get,
            ENDPOINTS[1],
            PARTITION_PATH,
            (TOKEN_HEADER, "token123"),
        );
        assert_eq!(
            std::str::from_utf8(send.as_bytes()).unwrap(),
            "GET /latest/meta-data/services/partition HTTP/1.1\r\n\
             Host: [fd00:ec2::254]:80\r\n\
             X-aws-ec2-metadata-token: token123\r\n\
             Connection: close\r\n\r\n"
        );
    }

    #[test]
    fn build_request_replaces_previous_request() {
        let mut send = RequestBuf::new();
        // Recorded before any build, so a reallocation by the largest request is caught.
        let capacity = send.buf.capacity();
        assert!(capacity >= SEND_BUF_CAPACITY);
        // The largest request: IPv6 host and a maximum-size token.
        send.build(
            Method::Get,
            ENDPOINTS[1],
            PARTITION_PATH,
            (TOKEN_HEADER, &"t".repeat(MAX_BODY_BYTES)),
        );
        assert_eq!(send.buf.capacity(), capacity);
        send.build(
            Method::Put,
            ENDPOINTS[0],
            TOKEN_PATH,
            (TOKEN_TTL_HEADER, TOKEN_TTL_SECS),
        );
        // Only the new request remains; nothing of the long token is appended.
        assert_eq!(
            std::str::from_utf8(send.as_bytes()).unwrap(),
            "PUT /latest/api/token HTTP/1.1\r\n\
             Host: 169.254.169.254:80\r\n\
             X-aws-ec2-metadata-token-ttl-seconds: 60\r\n\
             Content-Length: 0\r\n\
             Connection: close\r\n\r\n"
        );
        // Neither request reallocated the send buffer.
        assert_eq!(send.buf.capacity(), capacity);
    }

    #[test]
    fn parse_token_trims() {
        assert_eq!(parse_token("token123\n").unwrap(), "token123");
    }

    #[test]
    fn parse_token_borrows_from_body() {
        let body = " token123\n";
        let token = parse_token(body).unwrap();
        // The trimmed token is a subslice of `body`, not a copy.
        assert_eq!(token.as_ptr(), body[1..].as_ptr());
        assert_eq!(token.len(), "token123".len());
    }

    #[test]
    fn parse_token_empty_is_malformed() {
        assert!(matches!(parse_token(" \n"), Err(Error::Malformed)));
    }

    #[test]
    fn parse_token_rejects_header_injection() {
        assert!(matches!(
            parse_token("abc\r\nX-Evil: 1"),
            Err(Error::Malformed)
        ));
    }

    #[tokio::test]
    async fn exchange_sends_built_request() {
        let mut imds = imds_with_get_request();
        let expected = String::from_utf8(imds.send.as_bytes().to_vec()).unwrap();
        let (result, request) = duplex_exchange(&mut imds, &http_response(OK, "aws"), 1024).await;
        assert_eq!(result.unwrap(), "aws");
        assert_eq!(request, expected);
    }

    #[tokio::test]
    async fn exchange_content_length_response() {
        let mut imds = imds_with_get_request();
        let (result, _) = duplex_exchange(&mut imds, &http_response(OK, "aws-cn\n"), 1024).await;
        // `exchange` returns the untrimmed body.
        assert_eq!(result.unwrap(), "aws-cn\n");
    }

    #[tokio::test]
    async fn exchange_response_split_across_reads() {
        let mut imds = imds_with_get_request();
        // A 3-byte duplex buffer splits the request and response into many small pieces.
        let (result, _) = duplex_exchange(&mut imds, &http_response(OK, "aws-us-gov"), 3).await;
        assert_eq!(result.unwrap(), "aws-us-gov");
    }

    #[tokio::test]
    async fn exchange_body_delimited_by_eof() {
        let mut imds = imds_with_get_request();
        let (result, _) = duplex_exchange(
            &mut imds,
            b"HTTP/1.1 200 OK\r\nServer: EC2ws\r\n\r\naws",
            1024,
        )
        .await;
        assert_eq!(result.unwrap(), "aws");
    }

    #[tokio::test]
    async fn exchange_truncated_at_eof_is_malformed() {
        let mut imds = imds_with_get_request();
        let (result, _) = duplex_exchange(
            &mut imds,
            b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\naws",
            1024,
        )
        .await;
        assert!(matches!(result, Err(Error::Malformed)));
    }

    #[tokio::test]
    async fn exchange_empty_response_is_malformed() {
        let mut imds = imds_with_get_request();
        let (result, _) = duplex_exchange(&mut imds, b"", 1024).await;
        assert!(matches!(result, Err(Error::Malformed)));
    }

    #[tokio::test]
    async fn exchange_non_200_is_status() {
        let mut imds = imds_with_get_request();
        let (result, _) = duplex_exchange(
            &mut imds,
            &http_response("HTTP/1.1 404 Not Found", ""),
            1024,
        )
        .await;
        assert!(matches!(result, Err(Error::Status(404))));
    }

    #[tokio::test]
    async fn exchange_oversized_body_is_too_large() {
        let mut imds = imds_with_get_request();
        let body = "a".repeat(MAX_BODY_BYTES + 1);
        let (result, _) = duplex_exchange(&mut imds, &http_response(OK, &body), 1024).await;
        assert!(matches!(result, Err(Error::ResponseTooLarge)));
    }

    #[tokio::test]
    async fn exchange_response_filling_buffer_is_too_large() {
        let mut imds = imds_with_get_request();
        // Headers alone fill the receive buffer, so the response never completes.
        let mut response = b"HTTP/1.1 200 OK\r\n".to_vec();
        while response.len() < MAX_RESPONSE_BYTES {
            response.extend_from_slice(b"X-Pad: 0\r\n");
        }
        let (result, _) = duplex_exchange(&mut imds, &response, 1024).await;
        assert!(matches!(result, Err(Error::ResponseTooLarge)));
    }

    #[tokio::test]
    async fn exchange_write_failure_is_io_and_wipes_send_buffer() {
        let mut imds = imds_with_get_request();
        let (mut client, server) = tokio::io::duplex(1024);
        drop(server);
        let result = Imds::exchange(&mut imds.send, &mut imds.recv, &mut client).await;
        assert!(matches!(result, Err(Error::Io(_))));
        assert!(imds.send.as_bytes().is_empty());
    }

    #[tokio::test]
    async fn exchange_zeroes_buffers_between_exchanges() {
        let mut imds = imds_with_get_request();
        // A long EOF-delimited body fills much of the receive buffer.
        let mut first = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        first.extend(std::iter::repeat_n(b'a', MAX_BODY_BYTES));
        let (result, _) = duplex_exchange(&mut imds, &first, 1024).await;
        assert_eq!(result.unwrap(), "a".repeat(MAX_BODY_BYTES));
        // The sent request was wiped from the send buffer.
        assert!(imds.send.as_bytes().is_empty());

        // A second, shorter EOF-delimited response must not pick up the first's bytes.
        imds.send.build(
            Method::Put,
            ENDPOINTS[0],
            TOKEN_PATH,
            (TOKEN_TTL_HEADER, TOKEN_TTL_SECS),
        );
        let second = b"HTTP/1.1 200 OK\r\n\r\nbb";
        let (result, request) = duplex_exchange(&mut imds, second, 1024).await;
        assert_eq!(result.unwrap(), "bb");
        assert!(request.starts_with("PUT /latest/api/token HTTP/1.1\r\n"));
        assert_eq!(&imds.recv.buf[..second.len()], second);
        assert!(imds.recv.buf[second.len()..].iter().all(|&b| b == 0));
        assert!(imds.send.as_bytes().is_empty());
    }

    #[tokio::test]
    async fn fetch_partition_happy_path_sends_token() {
        let server = MockServer::start().await;
        // Literal on-the-wire values (not the module constants), so a typo in a constant
        // is caught here.
        Mock::given(method("PUT"))
            .and(path("/latest/api/token"))
            .and(header("X-aws-ec2-metadata-token-ttl-seconds", "60"))
            .respond_with(ResponseTemplate::new(200).set_body_string("token123"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/latest/meta-data/services/partition"))
            .and(header("X-aws-ec2-metadata-token", "token123"))
            .respond_with(ResponseTemplate::new(200).set_body_string("aws"))
            .expect(1)
            .mount(&server)
            .await;
        mount_no_other_requests(&server).await;

        let result = Imds::new().fetch_partition_at(*server.address()).await;
        assert_eq!(result.unwrap(), Partition::Aws);
    }

    #[tokio::test]
    async fn fetch_partition_trims_body() {
        let server = MockServer::start().await;
        // The token is trimmed before it is echoed into the header: the partition mock
        // only matches `token123`, and anything else gets `401`.
        mount_token(&server, "token123\n").await;
        mount_partition(
            &server,
            "token123",
            ResponseTemplate::new(200).set_body_string("aws-cn\n"),
        )
        .await;

        let result = Imds::new().fetch_partition_at(*server.address()).await;
        assert_eq!(result.unwrap(), Partition::AwsCn);
    }

    #[tokio::test]
    async fn fetch_partition_unknown_partition_is_aws() {
        let server = MockServer::start().await;
        mount_imds(&server, "token123", "aws-iso-b").await;

        let result = Imds::new().fetch_partition_at(*server.address()).await;
        assert_eq!(result.unwrap(), Partition::Aws);
    }

    #[tokio::test]
    async fn fetch_partition_token_rejected_is_status() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(TOKEN_PATH))
            .and(header(TOKEN_TTL_HEADER, TOKEN_TTL_SECS))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount(&server)
            .await;
        // The partition GET is never sent.
        mount_no_other_requests(&server).await;

        let result = Imds::new().fetch_partition_at(*server.address()).await;
        assert!(matches!(result, Err(Error::Status(403))));
    }

    #[tokio::test]
    async fn fetch_partition_metadata_not_found_is_status() {
        let server = MockServer::start().await;
        mount_token(&server, "token123").await;
        mount_partition(&server, "token123", ResponseTemplate::new(404)).await;

        let result = Imds::new().fetch_partition_at(*server.address()).await;
        assert!(matches!(result, Err(Error::Status(404))));
    }

    #[tokio::test]
    async fn fetch_partition_rejects_header_injection_in_token() {
        let server = MockServer::start().await;
        mount_token(&server, "abc\r\nX-Evil: 1").await;
        // The partition GET is never sent.
        mount_no_other_requests(&server).await;

        let result = Imds::new().fetch_partition_at(*server.address()).await;
        assert!(matches!(result, Err(Error::Malformed)));
    }

    #[test]
    fn endpoints_are_ipv4_then_ipv6() {
        assert_eq!(ENDPOINTS[0].to_string(), "169.254.169.254:80");
        assert_eq!(ENDPOINTS[1].to_string(), "[fd00:ec2::254]:80");
    }

    #[tokio::test]
    async fn fetch_partition_from_falls_back_after_refused() {
        let server = MockServer::start().await;
        mount_imds(&server, "token123", "aws-cn").await;
        let endpoints = [refused_addr(), *server.address()];

        let result = Imds::new().fetch_partition_from(&endpoints).await;
        assert_eq!(result.unwrap(), Partition::AwsCn);
    }

    #[tokio::test]
    async fn fetch_partition_from_falls_back_after_error_status() {
        let failing = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(TOKEN_PATH))
            .and(header(TOKEN_TTL_HEADER, TOKEN_TTL_SECS))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&failing)
            .await;
        mount_no_other_requests(&failing).await;
        let server = MockServer::start().await;
        mount_imds(&server, "token123", "aws").await;
        let endpoints = [*failing.address(), *server.address()];

        let result = Imds::new().fetch_partition_from(&endpoints).await;
        assert_eq!(result.unwrap(), Partition::Aws);
    }

    #[tokio::test]
    async fn fetch_partition_from_stops_at_first_success() {
        let server = MockServer::start().await;
        mount_imds(&server, "token123", "aws-cn").await;
        // The second endpoint must never be tried.
        let unused = MockServer::start().await;
        mount_no_other_requests(&unused).await;
        let endpoints = [*server.address(), *unused.address()];

        let result = Imds::new().fetch_partition_from(&endpoints).await;
        assert_eq!(result.unwrap(), Partition::AwsCn);
    }

    #[tokio::test]
    async fn fetch_partition_from_returns_last_error() {
        let forbidden = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(TOKEN_PATH))
            .and(header(TOKEN_TTL_HEADER, TOKEN_TTL_SECS))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount(&forbidden)
            .await;
        mount_no_other_requests(&forbidden).await;
        let failing = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(TOKEN_PATH))
            .and(header(TOKEN_TTL_HEADER, TOKEN_TTL_SECS))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&failing)
            .await;
        mount_no_other_requests(&failing).await;
        let endpoints = [*forbidden.address(), *failing.address()];

        let result = Imds::new().fetch_partition_from(&endpoints).await;
        // The first endpoint's `Status(403)` is superseded by the second's `Status(500)`.
        assert!(matches!(result, Err(Error::Status(500))));
    }

    #[tokio::test]
    async fn fetch_partition_from_returns_last_error_when_refused() {
        let forbidden = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(TOKEN_PATH))
            .and(header(TOKEN_TTL_HEADER, TOKEN_TTL_SECS))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount(&forbidden)
            .await;
        mount_no_other_requests(&forbidden).await;
        let endpoints = [*forbidden.address(), refused_addr()];

        let result = Imds::new().fetch_partition_from(&endpoints).await;
        // The first endpoint's `Status(403)` is superseded by the second's IO error.
        assert!(matches!(result, Err(Error::Io(_))));
    }

    #[tokio::test]
    async fn fetch_partition_connection_refused_is_io() {
        let result = Imds::new().fetch_partition_at(refused_addr()).await;
        assert!(matches!(result, Err(Error::Io(_))));
    }

    /// Real time, not `start_paused`: the mock server runs on its own thread and runtime,
    /// so paused time would only fire the client's timeout before the server is ever
    /// reached. The delay keeps the response strictly after the client's deadline.
    #[tokio::test]
    async fn fetch_partition_times_out() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(TOKEN_PATH))
            .and(header(TOKEN_TTL_HEADER, TOKEN_TTL_SECS))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("token123")
                    .set_delay(REQUEST_TIMEOUT + Duration::from_millis(500)),
            )
            .expect(1)
            .mount(&server)
            .await;
        mount_no_other_requests(&server).await;

        let result = Imds::new().fetch_partition_at(*server.address()).await;
        assert!(matches!(result, Err(Error::Timeout)));
    }
}
