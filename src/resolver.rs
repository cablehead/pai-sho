//! Owned resolver - answers `*.ps.internal` from live surfaces.
//!
//! A small UDP DNS server. It is authoritative for one suffix
//! (`.ps.internal`) and answers A queries from the surface table:
//! `vibenv-ndyg.ps.internal` resolves to whatever address the surface named
//! `vibenv-ndyg` is projected at, and stops resolving when that surface goes
//! away. It does not recurse or forward. In both deployments the OS only ever
//! sends it names under the suffix (dnsmasq `server=/ps.internal/...` on Linux,
//! `/etc/resolver/ps.internal` on macOS), so anything else gets empty NOERROR.
//!
//! `.internal` is ICANN-reserved for private use, so the suffix can never
//! collide with a public name (unlike a bare `.ps`, the Palestinian ccTLD).
//! See docs/adr/0004-peer-surfaces.md and 0005.

use crate::peer::PeerManager;
use anyhow::{Context, Result};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{info, warn};

/// The suffix this resolver is authoritative for, without a leading dot.
/// `.internal` is ICANN-reserved for private use (never publicly delegated).
pub const SUFFIX: &str = "ps.internal";

/// Bind a UDP resolver at `listen` and answer `*.<SUFFIX>` from `peers`.
pub async fn run(listen: SocketAddr, peers: Arc<PeerManager>) -> Result<()> {
    let sock = UdpSocket::bind(listen)
        .await
        .with_context(|| format!("failed to bind resolver at {}", listen))?;
    info!("resolver listening on {} for *.{}", listen, SUFFIX);

    let mut buf = [0u8; 512];
    loop {
        let (n, from) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                warn!("resolver recv error: {}", e);
                continue;
            }
        };
        let reply = match handle_query(&buf[..n], &peers).await {
            Some(reply) => reply,
            None => continue,
        };
        if let Err(e) = sock.send_to(&reply, from).await {
            warn!("resolver send error: {}", e);
        }
    }
}

/// Build a reply for one query packet, or None if it is malformed.
async fn handle_query(query: &[u8], peers: &Arc<PeerManager>) -> Option<Vec<u8>> {
    let q = parse_question(query)?;
    let answer = if q.qtype == TYPE_A {
        match host_label(&q.name) {
            Some(label) => match peers.resolve_name(label).await {
                Some(IpAddr::V4(v4)) => Some(v4),
                _ => None,
            },
            None => None,
        }
    } else {
        // A name we own may exist without an A record (e.g. AAAA); answer
        // empty NOERROR rather than NXDOMAIN so the client does not cache a
        // negative for the whole name.
        None
    };
    Some(build_reply(query, q.qend, answer))
}

/// Build a reply for `query` synchronously, resolving `.ps.internal` names through
/// `resolve`. Used by the in-stack (TUN) resolver, which holds the name->ip
/// map directly. Returns None only if the query is malformed.
pub fn reply<F: FnOnce(&str) -> Option<std::net::Ipv4Addr>>(
    query: &[u8],
    resolve: F,
) -> Option<Vec<u8>> {
    let q = parse_question(query)?;
    let answer = if q.qtype == TYPE_A {
        host_label(&q.name).and_then(resolve)
    } else {
        None
    };
    Some(build_reply(query, q.qend, answer))
}

const TYPE_A: u16 = 1;

struct Question {
    /// Lowercased dotted name, no trailing dot
    name: String,
    qtype: u16,
    /// Offset just past the question (name + qtype + qclass)
    qend: usize,
}

/// Parse the single question in a standard query. Returns None on anything
/// malformed or unsupported (no questions, compression in the question, etc).
fn parse_question(buf: &[u8]) -> Option<Question> {
    if buf.len() < 12 {
        return None;
    }
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    if qdcount != 1 {
        return None;
    }

    let mut i = 12;
    let mut labels: Vec<String> = Vec::new();
    loop {
        let len = *buf.get(i)? as usize;
        i += 1;
        if len == 0 {
            break;
        }
        if len & 0xC0 != 0 {
            // Compression pointer in a question is not expected; bail.
            return None;
        }
        let end = i + len;
        let label = buf.get(i..end)?;
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        i = end;
    }
    let qtype = u16::from_be_bytes([*buf.get(i)?, *buf.get(i + 1)?]);
    // qclass at i+2..i+4, ignored
    let qend = i + 4;
    if buf.len() < qend {
        return None;
    }

    Some(Question {
        name: labels.join("."),
        qtype,
        qend,
    })
}

/// If `name` is `<label>.<SUFFIX>` with a single label, return `<label>`.
fn host_label(name: &str) -> Option<&str> {
    let rest = name.strip_suffix(SUFFIX)?.strip_suffix('.')?;
    if rest.is_empty() || rest.contains('.') {
        return None;
    }
    Some(rest)
}

/// Assemble a reply that echoes the question and carries an A record when
/// `answer` is Some; otherwise an empty NOERROR.
fn build_reply(query: &[u8], qend: usize, answer: Option<Ipv4Addr>) -> Vec<u8> {
    let mut out = Vec::with_capacity(qend + 16);

    // Header: id echoed; QR=1, opcode 0, AA=1, RD copied from query; RA=0,
    // RCODE=0 (NOERROR).
    out.extend_from_slice(&query[0..2]);
    let rd = query[2] & 0x01;
    out.push(0x80 | 0x04 | rd); // QR | AA | RD
    out.push(0x00);
    out.extend_from_slice(&[0x00, 0x01]); // QDCOUNT = 1
    let ancount: u16 = if answer.is_some() { 1 } else { 0 };
    out.extend_from_slice(&ancount.to_be_bytes());
    out.extend_from_slice(&[0x00, 0x00]); // NSCOUNT
    out.extend_from_slice(&[0x00, 0x00]); // ARCOUNT

    // Question, copied verbatim
    out.extend_from_slice(&query[12..qend]);

    if let Some(ip) = answer {
        out.extend_from_slice(&[0xC0, 0x0C]); // name pointer to the question
        out.extend_from_slice(&TYPE_A.to_be_bytes());
        out.extend_from_slice(&[0x00, 0x01]); // class IN
        out.extend_from_slice(&30u32.to_be_bytes()); // TTL 30s
        out.extend_from_slice(&[0x00, 0x04]); // RDLENGTH
        out.extend_from_slice(&ip.octets());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal A-record query for a given name, id 0x1234, RD set.
    fn query_for(name: &str) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        for label in name.split('.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&TYPE_A.to_be_bytes());
        q.extend_from_slice(&[0x00, 0x01]);
        q
    }

    #[test]
    fn parses_a_query() {
        let q = parse_question(&query_for("vibenv-ndyg.ps.internal")).unwrap();
        assert_eq!(q.name, "vibenv-ndyg.ps.internal");
        assert_eq!(q.qtype, TYPE_A);
    }

    #[test]
    fn host_label_strips_suffix() {
        assert_eq!(host_label("vibenv-ndyg.ps.internal"), Some("vibenv-ndyg"));
        assert_eq!(host_label("broker.ps.internal"), Some("broker"));
        assert_eq!(host_label("a.b.ps.internal"), None); // only single-label names
        assert_eq!(host_label("nope.com"), None);
        assert_eq!(host_label("ps.internal"), None);
    }

    #[test]
    fn reply_carries_the_answer() {
        let query = query_for("broker.ps.internal");
        let q = parse_question(&query).unwrap();
        let reply = build_reply(&query, q.qend, Some(Ipv4Addr::new(127, 0, 1, 5)));
        // header: QR set, ANCOUNT 1
        assert_eq!(reply[2] & 0x80, 0x80);
        assert_eq!(u16::from_be_bytes([reply[6], reply[7]]), 1);
        // last four bytes are the A record address
        let n = reply.len();
        assert_eq!(&reply[n - 4..], &[127, 0, 1, 5]);
    }

    #[test]
    fn empty_answer_has_no_records() {
        let query = query_for("gone.ps.internal");
        let q = parse_question(&query).unwrap();
        let reply = build_reply(&query, q.qend, None);
        assert_eq!(u16::from_be_bytes([reply[6], reply[7]]), 0); // ANCOUNT 0
    }
}
