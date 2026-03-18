use super::ShellOutput;

pub async fn cmd_ifconfig(args: &[&str]) -> ShellOutput {
    let filter_iface = args.first().filter(|a| !a.starts_with('-')).copied();

    #[cfg(target_os = "linux")]
    {
        let mut out = String::new();
        let interfaces = list_interfaces();

        for iface in &interfaces {
            if let Some(filter) = filter_iface {
                if iface.name != filter {
                    continue;
                }
            }
            out.push_str(&format!(
                "\x1b[1m{}\x1b[0m: flags=<{}> mtu {}\r\n",
                iface.name,
                if iface.up { "UP" } else { "DOWN" },
                iface.mtu
            ));
            if let Some(ref ip) = iface.ipv4 {
                out.push_str(&format!(
                    "        inet {} netmask {}\r\n",
                    ip,
                    iface.netmask.as_deref().unwrap_or("?")
                ));
            }
            for ip6 in &iface.ipv6 {
                out.push_str(&format!("        inet6 {}\r\n", ip6));
            }
            if let Some(ref mac) = iface.mac {
                out.push_str(&format!("        ether {}\r\n", mac));
            }
            out.push_str(&format!(
                "        RX bytes:{} TX bytes:{}\r\n",
                super::file::format_size_human(iface.rx_bytes),
                super::file::format_size_human(iface.tx_bytes),
            ));
            out.push_str("\r\n");
        }

        if out.is_empty() {
            out = "No interfaces found\r\n".to_string();
        }
        ShellOutput::text(out)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = filter_iface;
        ShellOutput::text("ifconfig: not available (Linux only)\r\n")
    }
}

#[cfg(target_os = "linux")]
struct InterfaceInfo {
    name: String,
    up: bool,
    mtu: String,
    mac: Option<String>,
    ipv4: Option<String>,
    netmask: Option<String>,
    ipv6: Vec<String>,
    rx_bytes: u64,
    tx_bytes: u64,
}

#[cfg(target_os = "linux")]
fn list_interfaces() -> Vec<InterfaceInfo> {
    let mut interfaces = Vec::new();

    let entries = match std::fs::read_dir("/sys/class/net") {
        Ok(e) => e,
        Err(_) => return interfaces,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let base = format!("/sys/class/net/{}", name);

        let mac = read_sysfs(&format!("{}/address", base));
        let mtu = read_sysfs(&format!("{}/mtu", base)).unwrap_or_else(|| "?".into());
        let operstate = read_sysfs(&format!("{}/operstate", base)).unwrap_or_default();
        let up = operstate == "up" || operstate == "unknown";
        let rx_bytes: u64 = read_sysfs(&format!("{}/statistics/rx_bytes", base))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let tx_bytes: u64 = read_sysfs(&format!("{}/statistics/tx_bytes", base))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // Get IPv4 via ioctl
        let ipv4 = get_interface_ipv4(&name);
        let netmask = get_interface_netmask(&name);

        // Get IPv6 from /proc/net/if_inet6
        let ipv6 = get_interface_ipv6(&name);

        interfaces.push(InterfaceInfo {
            name,
            up,
            mtu,
            mac,
            ipv4,
            netmask,
            ipv6,
            rx_bytes,
            tx_bytes,
        });
    }

    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    interfaces
}

#[cfg(target_os = "linux")]
fn read_sysfs(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(target_os = "linux")]
fn get_interface_ipv4(ifname: &str) -> Option<String> {
    use std::ffi::CString;
    let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if sock < 0 {
        return None;
    }
    let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
    let name = CString::new(ifname).ok()?;
    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(libc::IFNAMSIZ - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(
            name_bytes.as_ptr(),
            ifr.ifr_name.as_mut_ptr() as *mut u8,
            copy_len,
        );
    }
    let ret = unsafe { libc::ioctl(sock, libc::SIOCGIFADDR as _, &mut ifr) };
    unsafe {
        libc::close(sock);
    }
    if ret < 0 {
        return None;
    }
    let addr =
        unsafe { &*(&ifr.ifr_ifru as *const _ as *const libc::sockaddr_in) };
    let ip_bytes = addr.sin_addr.s_addr.to_ne_bytes();
    Some(format!(
        "{}.{}.{}.{}",
        ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]
    ))
}

#[cfg(target_os = "linux")]
fn get_interface_netmask(ifname: &str) -> Option<String> {
    use std::ffi::CString;
    let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if sock < 0 {
        return None;
    }
    let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
    let name = CString::new(ifname).ok()?;
    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(libc::IFNAMSIZ - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(
            name_bytes.as_ptr(),
            ifr.ifr_name.as_mut_ptr() as *mut u8,
            copy_len,
        );
    }
    let ret = unsafe { libc::ioctl(sock, libc::SIOCGIFNETMASK as _, &mut ifr) };
    unsafe {
        libc::close(sock);
    }
    if ret < 0 {
        return None;
    }
    let addr =
        unsafe { &*(&ifr.ifr_ifru as *const _ as *const libc::sockaddr_in) };
    let ip_bytes = addr.sin_addr.s_addr.to_ne_bytes();
    Some(format!(
        "{}.{}.{}.{}",
        ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]
    ))
}

#[cfg(target_os = "linux")]
fn get_interface_ipv6(ifname: &str) -> Vec<String> {
    let mut addrs = Vec::new();
    let content = match std::fs::read_to_string("/proc/net/if_inet6") {
        Ok(c) => c,
        Err(_) => return addrs,
    };
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 6 && parts[5] == ifname {
            let hex = parts[0];
            if hex.len() == 32 {
                let formatted = format!(
                    "{}:{}:{}:{}:{}:{}:{}:{}",
                    &hex[0..4],
                    &hex[4..8],
                    &hex[8..12],
                    &hex[12..16],
                    &hex[16..20],
                    &hex[20..24],
                    &hex[24..28],
                    &hex[28..32]
                );
                let prefix_len = parts[2];
                let prefix: u32 = u32::from_str_radix(prefix_len, 16).unwrap_or(0);
                addrs.push(format!("{}/{}", formatted, prefix));
            }
        }
    }
    addrs
}

pub async fn cmd_ip(args: &[&str]) -> ShellOutput {
    let subcmd = args.first().copied().unwrap_or("addr");

    match subcmd {
        "addr" | "a" | "address" => cmd_ifconfig(&args[1..]).await,
        "link" | "l" => cmd_ifconfig(&args[1..]).await,
        "route" | "r" => cmd_route(&args[1..]).await,
        _ => ShellOutput::text(format!(
            "ip: unknown subcommand '{}'\r\nUsage: ip {{addr|link|route}}\r\n",
            subcmd
        )),
    }
}

pub async fn cmd_ping(args: &[&str]) -> ShellOutput {
    let mut count = 4u32;
    let mut host = "";

    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-c" if i + 1 < args.len() => {
                count = args[i + 1].parse().unwrap_or(4);
                i += 1;
            }
            _ if !args[i].starts_with('-') => host = args[i],
            _ => {}
        }
        i += 1;
    }

    if host.is_empty() {
        return ShellOutput::text("usage: ping [-c count] <host>\r\n");
    }

    // Resolve hostname
    let addrs: Vec<std::net::SocketAddr> =
        match tokio::net::lookup_host(format!("{}:80", host)).await {
            Ok(a) => a.collect(),
            Err(e) => return ShellOutput::text(format!("ping: {}: {}\r\n", host, e)),
        };

    let addr = match addrs.first() {
        Some(a) => *a,
        None => {
            return ShellOutput::text(format!(
                "ping: {}: Name or service not known\r\n",
                host
            ))
        }
    };

    let mut out = String::new();
    out.push_str(&format!("PING {} ({}) — TCP probe\r\n", host, addr.ip()));

    let mut sent = 0u32;
    let mut success = 0u32;
    let mut rtts = Vec::new();

    for seq in 0..count {
        sent += 1;
        let start = std::time::Instant::now();
        let result = tokio::time::timeout(
            tokio::time::Duration::from_secs(2),
            tokio::net::TcpStream::connect(addr),
        )
        .await;
        let elapsed = start.elapsed();

        match result {
            Ok(Ok(_stream)) => {
                let rtt = elapsed.as_secs_f64() * 1000.0;
                success += 1;
                rtts.push(rtt);
                out.push_str(&format!(
                    "Connected to {}: seq={} time={:.1}ms\r\n",
                    addr.ip(),
                    seq,
                    rtt
                ));
            }
            Ok(Err(e)) => {
                // Connection refused still means host is reachable
                let rtt = elapsed.as_secs_f64() * 1000.0;
                let msg = e.to_string();
                if msg.contains("refused") {
                    success += 1;
                    rtts.push(rtt);
                    out.push_str(&format!(
                        "Host reachable (port closed): seq={} time={:.1}ms\r\n",
                        seq, rtt
                    ));
                } else {
                    out.push_str(&format!("seq={}: {}\r\n", seq, e));
                }
            }
            Err(_) => {
                out.push_str(&format!("seq={}: timeout\r\n", seq));
            }
        }

        if seq + 1 < count {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    let loss = if sent > 0 {
        ((sent - success) as f64 / sent as f64) * 100.0
    } else {
        100.0
    };
    out.push_str(&format!("\r\n--- {} ping statistics ---\r\n", host));
    out.push_str(&format!(
        "{} probes, {} successful, {:.0}% loss\r\n",
        sent, success, loss
    ));
    if !rtts.is_empty() {
        let min = rtts.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = rtts.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let avg = rtts.iter().sum::<f64>() / rtts.len() as f64;
        out.push_str(&format!(
            "rtt min/avg/max = {:.1}/{:.1}/{:.1} ms\r\n",
            min, avg, max
        ));
    }

    ShellOutput::text(out)
}

pub async fn cmd_curl(args: &[&str]) -> ShellOutput {
    let mut url = "";
    let mut show_headers = false;
    let mut method = "GET";
    let mut output_file = None;

    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-I" | "--head" => {
                show_headers = true;
                method = "HEAD";
            }
            "-X" if i + 1 < args.len() => {
                method = args[i + 1];
                i += 1;
            }
            "-o" if i + 1 < args.len() => {
                output_file = Some(args[i + 1]);
                i += 1;
            }
            "-s" | "--silent" => {} // Already silent
            _ if !args[i].starts_with('-') && url.is_empty() => url = args[i],
            _ => {}
        }
        i += 1;
    }

    if url.is_empty() {
        return ShellOutput::text("usage: curl [-I] [-X METHOD] [-o file] <url>\r\n");
    }

    // Ensure URL has scheme
    let url = if !url.starts_with("http://") && !url.starts_with("https://") {
        format!("https://{}", url)
    } else {
        url.to_string()
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    let result = match method {
        "HEAD" => client.head(&url).send().await,
        "POST" => client.post(&url).send().await,
        "PUT" => client.put(&url).send().await,
        "DELETE" => client.delete(&url).send().await,
        _ => client.get(&url).send().await,
    };

    match result {
        Ok(resp) => {
            if show_headers {
                let mut out = format!("HTTP/1.1 {}\r\n", resp.status());
                for (k, v) in resp.headers() {
                    out.push_str(&format!(
                        "{}: {}\r\n",
                        k,
                        v.to_str().unwrap_or("?")
                    ));
                }
                ShellOutput::text(out)
            } else if let Some(path) = output_file {
                match resp.bytes().await {
                    Ok(data) => match tokio::fs::write(path, &data).await {
                        Ok(()) => ShellOutput::text(format!(
                            "saved {} bytes to {}\r\n",
                            data.len(),
                            path
                        )),
                        Err(e) => ShellOutput::text(format!("curl: write error: {}\r\n", e)),
                    },
                    Err(e) => ShellOutput::text(format!("curl: {}\r\n", e)),
                }
            } else {
                match resp.text().await {
                    Ok(body) => {
                        let mut text = body.replace('\n', "\r\n");
                        if !text.ends_with("\r\n") {
                            text.push_str("\r\n");
                        }
                        ShellOutput::text(text)
                    }
                    Err(e) => ShellOutput::text(format!("curl: {}\r\n", e)),
                }
            }
        }
        Err(e) => ShellOutput::text(format!("curl: {}\r\n", e)),
    }
}

pub async fn cmd_netstat(args: &[&str]) -> ShellOutput {
    let _ = args;
    #[cfg(target_os = "linux")]
    {
        let mut out = String::new();
        out.push_str(&format!(
            "\x1b[1m{:<6} {:<24} {:<24} {}\x1b[0m\r\n",
            "Proto", "Local Address", "Foreign Address", "State"
        ));

        // Parse /proc/net/tcp
        if let Ok(content) = std::fs::read_to_string("/proc/net/tcp") {
            for line in content.lines().skip(1) {
                if let Some(entry) = parse_proc_net_tcp(line, "tcp") {
                    out.push_str(&entry);
                }
            }
        }
        // Parse /proc/net/tcp6
        if let Ok(content) = std::fs::read_to_string("/proc/net/tcp6") {
            for line in content.lines().skip(1) {
                if let Some(entry) = parse_proc_net_tcp6(line, "tcp6") {
                    out.push_str(&entry);
                }
            }
        }
        // Parse /proc/net/udp
        if let Ok(content) = std::fs::read_to_string("/proc/net/udp") {
            for line in content.lines().skip(1) {
                if let Some(entry) = parse_proc_net_tcp(line, "udp") {
                    out.push_str(&entry);
                }
            }
        }

        ShellOutput::text(out)
    }

    #[cfg(not(target_os = "linux"))]
    ShellOutput::text("netstat: not available (Linux only)\r\n")
}

pub async fn cmd_ss(args: &[&str]) -> ShellOutput {
    cmd_netstat(args).await
}

#[cfg(target_os = "linux")]
fn parse_proc_net_tcp(line: &str, proto: &str) -> Option<String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let local = parse_hex_addr(parts[1])?;
    let remote = parse_hex_addr(parts[2])?;
    let state_hex = parts[3];
    let state = tcp_state(state_hex);

    Some(format!(
        "{:<6} {:<24} {:<24} {}\r\n",
        proto, local, remote, state
    ))
}

#[cfg(target_os = "linux")]
fn parse_proc_net_tcp6(line: &str, proto: &str) -> Option<String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let local = parse_hex_addr6(parts[1])?;
    let remote = parse_hex_addr6(parts[2])?;
    let state_hex = parts[3];
    let state = tcp_state(state_hex);

    Some(format!(
        "{:<6} {:<24} {:<24} {}\r\n",
        proto, local, remote, state
    ))
}

#[cfg(target_os = "linux")]
fn parse_hex_addr(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let ip = u32::from_str_radix(parts[0], 16).ok()?;
    let port = u16::from_str_radix(parts[1], 16).ok()?;
    let bytes = ip.to_ne_bytes();
    Some(format!(
        "{}.{}.{}.{}:{}",
        bytes[0], bytes[1], bytes[2], bytes[3], port
    ))
}

#[cfg(target_os = "linux")]
fn parse_hex_addr6(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let port = u16::from_str_radix(parts[1], 16).ok()?;
    let hex = parts[0];

    // Check if it's an IPv4-mapped address (::ffff:x.x.x.x)
    if hex.len() == 32 && hex.starts_with("0000000000000000FFFF0000") {
        let ip_hex = &hex[24..32];
        let ip = u32::from_str_radix(ip_hex, 16).ok()?;
        let bytes = ip.to_ne_bytes();
        return Some(format!(
            "{}.{}.{}.{}:{}",
            bytes[0], bytes[1], bytes[2], bytes[3], port
        ));
    }

    // Full IPv6 - abbreviate
    if hex == "00000000000000000000000000000000" {
        return Some(format!("::::{}", port));
    }

    Some(format!("[{:.8}..]:{}",  hex, port))
}

#[cfg(target_os = "linux")]
fn tcp_state(hex: &str) -> &'static str {
    match hex {
        "01" => "ESTABLISHED",
        "02" => "SYN_SENT",
        "03" => "SYN_RECV",
        "04" => "FIN_WAIT1",
        "05" => "FIN_WAIT2",
        "06" => "TIME_WAIT",
        "07" => "CLOSE",
        "08" => "CLOSE_WAIT",
        "09" => "LAST_ACK",
        "0A" => "LISTEN",
        "0B" => "CLOSING",
        _ => "UNKNOWN",
    }
}

pub async fn cmd_nslookup(args: &[&str]) -> ShellOutput {
    if args.is_empty() {
        return ShellOutput::text("usage: nslookup <hostname>\r\n");
    }
    let host = args[0];

    match tokio::net::lookup_host(format!("{}:0", host)).await {
        Ok(addrs) => {
            let mut out = format!("Name:    {}\r\n", host);
            let mut found = false;
            for addr in addrs {
                out.push_str(&format!("Address: {}\r\n", addr.ip()));
                found = true;
            }
            if !found {
                out.push_str("(no addresses found)\r\n");
            }
            ShellOutput::text(out)
        }
        Err(e) => ShellOutput::text(format!(
            "** server can't find {}: {}\r\n",
            host, e
        )),
    }
}

pub fn cmd_hostname(container_name: &str, args: &[&str]) -> ShellOutput {
    let fqdn = args.iter().any(|a| *a == "-f" || *a == "--fqdn");
    if fqdn {
        #[cfg(target_os = "linux")]
        {
            if let Ok(name) = std::fs::read_to_string("/etc/hostname") {
                let name = name.trim();
                if !name.is_empty() {
                    return ShellOutput::text(format!("{}\r\n", name));
                }
            }
        }
    }
    ShellOutput::text(format!("{}\r\n", container_name))
}

pub async fn cmd_route(args: &[&str]) -> ShellOutput {
    let _ = args;
    #[cfg(target_os = "linux")]
    {
        let content = match std::fs::read_to_string("/proc/net/route") {
            Ok(c) => c,
            Err(e) => return ShellOutput::text(format!("route: {}\r\n", e)),
        };

        let mut out = String::new();
        out.push_str(&format!(
            "\x1b[1m{:<12} {:<16} {:<16} {:<6} {:<6} {:<6} {}\x1b[0m\r\n",
            "Iface", "Destination", "Gateway", "Flags", "Metric", "Ref", "Use"
        ));

        for line in content.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 8 {
                continue;
            }
            let iface = parts[0];
            let dest = hex_to_ip(parts[1]);
            let gw = hex_to_ip(parts[2]);
            let flags = parts[3];
            let metric = parts[6];

            let flag_str = {
                let f: u32 = flags.parse().unwrap_or(0);
                let mut s = String::new();
                if f & 0x0001 != 0 {
                    s.push('U');
                }
                if f & 0x0002 != 0 {
                    s.push('G');
                }
                if f & 0x0004 != 0 {
                    s.push('H');
                }
                s
            };

            out.push_str(&format!(
                "{:<12} {:<16} {:<16} {:<6} {:<6} {:<6} {}\r\n",
                iface, dest, gw, flag_str, metric, "0", "0"
            ));
        }

        ShellOutput::text(out)
    }

    #[cfg(not(target_os = "linux"))]
    ShellOutput::text("route: not available (Linux only)\r\n")
}

#[cfg(target_os = "linux")]
fn hex_to_ip(hex: &str) -> String {
    if let Ok(val) = u32::from_str_radix(hex, 16) {
        let bytes = val.to_ne_bytes();
        format!(
            "{}.{}.{}.{}",
            bytes[0], bytes[1], bytes[2], bytes[3]
        )
    } else {
        hex.to_string()
    }
}
