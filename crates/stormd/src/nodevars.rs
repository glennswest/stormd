//! Values a config cannot know until the node it runs on exists.
//!
//! A container image is built once and runs on every node, so anything that
//! names *this* node — its address above all — cannot be written into the
//! config. The usual answers are a wrapper script that computes it and execs,
//! or an operator that templates the config before starting it; both mean a
//! second program in the image whose only job is to fill in one blank.
//!
//! stormd is already the program that starts the process, so it fills them in.
//! `${NODE_IP}` in an argument or an environment value becomes this node's
//! address at the moment the process is spawned — not at build time, not at
//! boot, but each time it starts, so a node that changes address gets the new
//! one on the next restart.
//!
//! This is what a control plane needs to be a *cluster*. An apiserver told to
//! advertise 127.0.0.1 registers loopback as its endpoint and every peer that
//! reads it talks to itself; a leader election run against a loopback
//! apiserver elects a leader per node, so three masters are three leaders and
//! none of them is wrong from where it is standing. The address has to be the
//! one another node could dial, and only the node knows it.

use std::collections::HashMap;
use std::net::{IpAddr, UdpSocket};

/// This node's address: the source address of a route off the node.
///
/// No packet is sent — a connected UDP socket only asks the routing table
/// which local address would be used — so this needs no network to be up
/// beyond an address and a route, and cannot block.
///
/// The route to a public address rather than a fixed guess, because a node
/// with several interfaces has several addresses and the one that matters is
/// the one it reaches other nodes on.
pub fn node_ip() -> Option<IpAddr> {
    for probe in ["1.1.1.1:53", "192.168.1.1:53", "10.0.0.1:53"] {
        let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { continue };
        if sock.connect(probe).is_ok() {
            if let Ok(local) = sock.local_addr() {
                if !local.ip().is_unspecified() && !local.ip().is_loopback() {
                    return Some(local.ip());
                }
            }
        }
    }
    None
}

/// The variables a process may refer to.
pub fn vars() -> HashMap<String, String> {
    let mut v = HashMap::new();
    if let Some(ip) = node_ip() {
        v.insert("NODE_IP".into(), ip.to_string());
    }
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            v.insert("NODE_NAME".into(), h.to_owned());
        }
    }
    v
}

/// Replace `${NAME}` with what the node says it is.
///
/// A name with no value is left exactly as written rather than blanked. An
/// argument that reads `--advertise-address ${NODE_IP}` is a visible,
/// searchable failure; one that reads `--advertise-address` followed by
/// nothing is a parse error three layers down, and one silently replaced by an
/// empty string is a server advertising nothing at all.
pub fn expand(s: &str, vars: &HashMap<String, String>) -> String {
    if !s.contains("${") {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 2..];
        match tail.find('}') {
            Some(end) => {
                let name = &tail[..end];
                match vars.get(name) {
                    Some(v) => out.push_str(v),
                    None => {
                        out.push_str("${");
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &tail[end + 1..];
            }
            // An unclosed `${` is text, not a variable.
            None => {
                out.push_str("${");
                rest = tail;
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> HashMap<String, String> {
        let mut v = HashMap::new();
        v.insert("NODE_IP".to_string(), "192.168.8.104".to_string());
        v
    }

    #[test]
    fn a_known_name_is_replaced() {
        assert_eq!(expand("${NODE_IP}", &vars()), "192.168.8.104");
        assert_eq!(expand("https://${NODE_IP}:6443", &vars()), "https://192.168.8.104:6443");
        assert_eq!(expand("${NODE_IP}/${NODE_IP}", &vars()), "192.168.8.104/192.168.8.104");
    }

    #[test]
    fn an_unknown_name_survives_unchanged() {
        // Visible and searchable, rather than an empty argument that fails
        // three layers down as something else.
        assert_eq!(expand("${NOPE}", &vars()), "${NOPE}");
        assert_eq!(expand("a ${NOPE} b", &vars()), "a ${NOPE} b");
    }

    #[test]
    fn text_that_is_not_a_variable_is_left_alone() {
        assert_eq!(expand("plain", &vars()), "plain");
        assert_eq!(expand("cost: $5", &vars()), "cost: $5");
        assert_eq!(expand("${unclosed", &vars()), "${unclosed");
    }
}
