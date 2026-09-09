//! One HTTP/1.1 request over loopback, and nothing else.
//!
//! This workspace carries no HTTP client and does not gain one for this. The
//! precedent is [`crate::memory`], which declares `sysctlbyname` itself rather
//! than taking a `libc` dependency "for four integers"; the same judgement
//! applies here, and more strongly, because the alternative is not one crate
//! but an async runtime underneath it. What rung 3a needs of HTTP is a request
//! line, a handful of headers, a `Content-Length`-framed body and a status
//! code, spoken to a socket **this process opened on 127.0.0.1 and told a child
//! of its own to bind**. There is no TLS, no redirect, no proxy, no chunked
//! transfer, no content negotiation and no cookie jar, because there is no
//! network - the whole argument for this rung
//! is that nothing leaves the machine.
//!
//! It is also what makes the rung testable without Python. A client built on a
//! runtime would need that runtime in the test binary; this one talks to a
//! [`std::net::TcpListener`] a test thread owns, which is how
//! [`crate::engines::flux`]'s tests drive a whole region through the protocol
//! on a machine with no sidecar installed at all.
//!
//! **`Connection: close`, always.** Keeping a connection alive would save a
//! loopback handshake on a request that takes ten to sixty seconds, which is
//! not a saving worth a
//! second framing path. Closing also means the body is delimited twice over -
//! by `Content-Length` when the peer sends one, and by end-of-stream when it
//! does not - so a sidecar written against this document cannot frame a reply
//! in a way this cannot read.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// The header the shared secret travels in. `mc` for this application, so a
/// sidecar built for the typesetter's `x-mt-token` cannot be driven by ours by
/// accident - the two protocols are not the same protocol.
pub const TOKEN_HEADER: &str = "x-mc-token";

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Send one request and read one response.
///
/// `timeout` bounds the connect and each individual read and write, which is
/// **not** the same as bounding the whole exchange: a peer that dribbles one
/// byte per interval keeps the call alive indefinitely. That is deliberate here
/// and it is the reason the caller also holds a wall-clock deadline of its own
/// ([`crate::sidecar::Client`]) - a render legitimately takes a minute, so a
/// total socket timeout tight enough to catch a hang would also kill the work,
/// and the thing that must not be absorbed silently is the *hang*, which the
/// caller's deadline and the child's exit status both catch.
pub fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&[u8]>,
    timeout: Duration,
) -> std::io::Result<Response> {
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    // Every request is one small write followed by one long wait. Nagle would
    // hold the tail of the header block waiting for more, which is latency
    // bought with nothing.
    let _ = stream.set_nodelay(true);

    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\n",
        addr
    );
    if let Some(token) = token {
        head.push_str(&format!("{TOKEN_HEADER}: {token}\r\n"));
    }
    match body {
        Some(bytes) => head
            .push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n\r\n", bytes.len())),
        None => head.push_str("Content-Length: 0\r\n\r\n"),
    }
    stream.write_all(head.as_bytes())?;
    if let Some(bytes) = body {
        stream.write_all(bytes)?;
    }
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    parse(&raw)
}

/// Split a response into its status and its body.
///
/// Header *values* are never read beyond `Content-Length`, so this does not
/// lower-case names, join continuations or unfold anything: a header this does
/// not understand is a header rung 3a does not use.
fn parse(raw: &[u8]) -> std::io::Result<Response> {
    let split = find(raw, b"\r\n\r\n")
        .ok_or_else(|| bad("the sidecar's reply has no header terminator"))?;
    let head = std::str::from_utf8(&raw[..split])
        .map_err(|_| bad("the sidecar's reply headers are not text"))?;
    let mut lines = head.split("\r\n");

    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| bad("the sidecar's reply has no status line"))?;

    let rest = &raw[split + 4..];
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok());

    // A declared length that overruns what arrived is a truncated reply, not a
    // reason to hand back a short buffer and let JSON blame itself.
    let body = match length {
        Some(n) if n > rest.len() => {
            return Err(bad("the sidecar's reply ended before its declared length"));
        }
        Some(n) => rest[..n].to_vec(),
        None => rest.to_vec(),
    };
    Ok(Response { status, body })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn bad(detail: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, detail.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_content_length_reply_is_read_to_its_declared_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}trailing";
        let reply = parse(raw).unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, b"{}");
    }

    /// The other framing, so a sidecar that closes rather than counting is
    /// still readable.
    #[test]
    fn a_reply_with_no_length_is_read_to_the_end_of_the_stream() {
        let raw = b"HTTP/1.1 507 Insufficient Storage\r\n\r\n{\"error\":{\"kind\":\"out_of_memory\"}}";
        let reply = parse(raw).unwrap();
        assert_eq!(reply.status, 507);
        assert!(String::from_utf8_lossy(&reply.body).contains("out_of_memory"));
    }

    /// A truncated reply is an error rather than a short body. Handing the
    /// short buffer on would make the JSON parser report a malformed document,
    /// which names the wrong fault.
    #[test]
    fn a_reply_shorter_than_its_own_header_says_is_refused() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 64\r\n\r\n{}";
        assert!(parse(raw).is_err());
    }

    #[test]
    fn a_reply_that_is_not_http_is_refused_rather_than_guessed_at() {
        assert!(parse(b"nonsense").is_err());
        assert!(parse(b"nonsense\r\n\r\nbody").is_err());
    }
}
