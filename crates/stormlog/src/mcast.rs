//! Emit to the fleet's multicast syslog group.
//!
//! Every process on a stormcos node — whether supervised by stormpump on the
//! host or by stormd inside a container — puts its lines on the same group in
//! the same framing. One wire format, so a viewer that can read one node can
//! read all of them, and a node needs no configuration to be watched: it emits
//! to a group and does not know or care who is listening.
//!
//! **Emitting only.** Receiving, storing, indexing and searching is a
//! collector's job, and doing it here as well would mean two stores, two
//! schemas, and a fleet view that sees half the nodes. A container init has no
//! business holding a log database.
//!
//! Nothing here can block or fail loudly. It is a datagram on a non-blocking
//! socket: a group nobody is listening to costs one `sendto` that goes nowhere,
//! and the line is in the local file regardless.

use crate::types::{LogEntry, Severity};
use std::net::{SocketAddr, UdpSocket};

/// The default group: administratively scoped, so it stays inside the site.
pub const DEFAULT_GROUP: &str = "239.255.42.1:5514";

/// Longest line put on the wire. Beyond this it is a payload, not a log.
const MAX_LINE: usize = 8 * 1024;

/// `local0`, matching what stormpump emits.
const FACILITY_LOCAL0: u8 = 16;

pub struct Emitter {
    sock: UdpSocket,
    addr: SocketAddr,
    host: String,
}

impl Emitter {
    /// Open the sink. A multicast group needs a TTL that will leave the node —
    /// the default of 1 does not cross a router — and loopback delivery on, so
    /// a collector running on the node itself sees the stream too.
    pub fn new(addr: SocketAddr, host: impl Into<String>) -> Option<Emitter> {
        let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
        sock.set_nonblocking(true).ok()?;
        if let std::net::IpAddr::V4(ip) = addr.ip() {
            if ip.is_multicast() {
                let _ = sock.set_multicast_ttl_v4(4);
                let _ = sock.set_multicast_loop_v4(true);
            }
        }
        Some(Emitter { sock, addr, host: host.into() })
    }

    /// RFC 5424, with the entry's own timestamp.
    ///
    /// The entry's, not the moment of sending: a backlog forwarded after the
    /// network came up would otherwise collapse onto one instant, and the
    /// ordering a viewer shows would be the order things were *sent* rather
    /// than the order they happened.
    pub fn send(&self, entry: &LogEntry) {
        let sev = severity_of(entry.severity);
        let pri = FACILITY_LOCAL0 * 8 + sev;
        let mut msg = strip_ansi(&entry.line);
        msg.truncate(MAX_LINE);
        let frame = format!(
            "<{pri}>1 {} {} {} - - - {msg}",
            entry.timestamp.format("%Y-%m-%dT%H:%M:%S%.6fZ"),
            self.host,
            entry.process,
        );
        // Best effort by construction: a datagram nobody takes is not this
        // node's problem, and the line is already in the local file.
        let _ = self.sock.send_to(frame.as_bytes(), self.addr);
    }
}

/// stormlog's severities onto syslog's numbers.
fn severity_of(s: Severity) -> u8 {
    match s {
        Severity::Emergency => 0,
        Severity::Alert => 1,
        Severity::Critical => 2,
        Severity::Error => 3,
        Severity::Warning => 4,
        Severity::Notice => 5,
        Severity::Info => 6,
        Severity::Debug => 7,
    }
}

/// Strip terminal escape sequences.
///
/// Anything that colours its output when it thinks a terminal is watching will
/// do so here, because a supervised process is given one. Those codes then
/// travel the whole way — file, wire, viewer — where they are not colour but
/// litter: they break a search for a word sitting next to one and they cost
/// bytes on every datagram.
pub fn strip_ansi(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b {
            i += 1;
            match b.get(i) {
                // CSI: ESC [ params… final byte in @..~
                Some(b'[') => {
                    i += 1;
                    while i < b.len() && !(0x40..=0x7e).contains(&b[i]) {
                        i += 1;
                    }
                    i += 1;
                }
                // OSC: ESC ] … BEL or ST. Title-setting sequences arrive this
                // way and are longer than a colour code.
                Some(b']') => {
                    i += 1;
                    while i < b.len() && b[i] != 0x07 {
                        if b[i] == 0x1b && b.get(i + 1) == Some(&b'\\') {
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                    i += 1;
                }
                Some(_) => i += 1,
                None => break,
            }
        } else {
            // Not byte-wise: this must stay valid UTF-8, and the input is a
            // &str, so the character starting here is whole.
            let n = s[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            out.push_str(&s[i..i + n]);
            i += n;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_codes_do_not_reach_the_wire() {
        let line = "\u{1b}[2m2026-08-25T11:50:09Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m up";
        assert_eq!(strip_ansi(line), "2026-08-25T11:50:09Z  INFO up");
        // A title-setting sequence, which ends in BEL rather than a letter.
        assert_eq!(strip_ansi("a\u{1b}]0;title\u{7}b"), "ab");
        // Multi-byte text survives intact — an em dash is three bytes and a
        // byte-wise copy would split it.
        assert_eq!(strip_ansi("plain — text"), "plain — text");
    }

    #[test]
    fn severities_match_the_syslog_numbers() {
        // local0.info is 134, which is what a collector will key on.
        assert_eq!(FACILITY_LOCAL0 * 8 + severity_of(Severity::Info), 134);
        assert_eq!(severity_of(Severity::Error), 3);
        assert_eq!(severity_of(Severity::Emergency), 0);
    }
}
