use super::ShellOutput;

/// Read input from file arg or piped input.
async fn get_input(args: &[&str], piped: Option<&str>, skip_flags: usize) -> Option<String> {
    // Find first non-flag argument after skip_flags
    let file_args: Vec<&&str> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .skip(skip_flags)
        .collect();

    if let Some(path) = file_args.first() {
        tokio::fs::read_to_string(path).await.ok()
    } else {
        piped.map(|s| s.replace("\r\n", "\n"))
    }
}

pub async fn cmd_grep(args: &[&str], piped: Option<&str>) -> ShellOutput {
    let mut ignore_case = false;
    let mut invert = false;
    let mut count_only = false;
    let mut line_numbers = false;
    let mut pattern = None;
    let mut files = Vec::new();

    for arg in args {
        match *arg {
            "-i" => ignore_case = true,
            "-v" => invert = true,
            "-c" => count_only = true,
            "-n" => line_numbers = true,
            "-iv" | "-vi" => {
                ignore_case = true;
                invert = true;
            }
            "-in" | "-ni" => {
                ignore_case = true;
                line_numbers = true;
            }
            _ if !arg.starts_with('-') => {
                if pattern.is_none() {
                    pattern = Some(*arg);
                } else {
                    files.push(*arg);
                }
            }
            _ => {}
        }
    }

    let pattern = match pattern {
        Some(p) => p,
        None => return ShellOutput::text("usage: grep [-ivc] <pattern> [file...]\r\n"),
    };

    let content = if !files.is_empty() {
        let mut all = String::new();
        for f in &files {
            match tokio::fs::read_to_string(f).await {
                Ok(c) => all.push_str(&c),
                Err(e) => return ShellOutput::text(format!("grep: {}: {}\r\n", f, e)),
            }
        }
        all
    } else if let Some(input) = piped {
        input.replace("\r\n", "\n")
    } else {
        return ShellOutput::text("usage: grep <pattern> <file>\r\n");
    };

    let pat_lower = pattern.to_lowercase();
    let mut matches = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let haystack = if ignore_case {
            line.to_lowercase()
        } else {
            line.to_string()
        };
        let needle = if ignore_case { &pat_lower } else { pattern };
        let found = haystack.contains(needle);
        let matched = if invert { !found } else { found };
        if matched {
            if line_numbers {
                matches.push(format!("{}:{}", i + 1, line));
            } else {
                matches.push(line.to_string());
            }
        }
    }

    if count_only {
        return ShellOutput::text(format!("{}\r\n", matches.len()));
    }

    if matches.is_empty() {
        return ShellOutput::text("");
    }
    ShellOutput::text(matches.join("\r\n") + "\r\n")
}

pub async fn cmd_sort(args: &[&str], piped: Option<&str>) -> ShellOutput {
    let mut reverse = false;
    let mut numeric = false;
    let mut unique = false;

    for arg in args {
        match *arg {
            "-r" => reverse = true,
            "-n" => numeric = true,
            "-u" => unique = true,
            "-rn" | "-nr" => {
                reverse = true;
                numeric = true;
            }
            "-ru" | "-ur" => {
                reverse = true;
                unique = true;
            }
            "-nu" | "-un" => {
                numeric = true;
                unique = true;
            }
            _ => {}
        }
    }

    let input = match get_input(args, piped, 0).await {
        Some(s) => s,
        None => return ShellOutput::text(""),
    };

    let mut lines: Vec<&str> = input.lines().collect();
    if numeric {
        lines.sort_by(|a, b| {
            let na: f64 = a.trim().parse().unwrap_or(0.0);
            let nb: f64 = b.trim().parse().unwrap_or(0.0);
            na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal)
        });
    } else {
        lines.sort();
    }
    if reverse {
        lines.reverse();
    }
    if unique {
        lines.dedup();
    }

    ShellOutput::text(lines.join("\r\n") + "\r\n")
}

pub async fn cmd_uniq(args: &[&str], piped: Option<&str>) -> ShellOutput {
    let mut count = false;
    for arg in args {
        if *arg == "-c" {
            count = true;
        }
    }

    let input = match get_input(args, piped, 0).await {
        Some(s) => s,
        None => return ShellOutput::text(""),
    };

    let lines: Vec<&str> = input.lines().collect();
    let mut out = String::new();

    if count {
        let mut i = 0;
        while i < lines.len() {
            let mut n = 1;
            while i + n < lines.len() && lines[i + n] == lines[i] {
                n += 1;
            }
            out.push_str(&format!("{:>7} {}\r\n", n, lines[i]));
            i += n;
        }
    } else {
        let mut prev: Option<&str> = None;
        for line in &lines {
            if prev != Some(line) {
                out.push_str(line);
                out.push_str("\r\n");
            }
            prev = Some(line);
        }
    }
    ShellOutput::text(out)
}

pub async fn cmd_cut(args: &[&str], piped: Option<&str>) -> ShellOutput {
    let mut delimiter = '\t';
    let mut fields: Vec<usize> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-d" if i + 1 < args.len() => {
                delimiter = args[i + 1].chars().next().unwrap_or('\t');
                i += 1;
            }
            s if s.starts_with("-d") => {
                delimiter = s[2..].chars().next().unwrap_or('\t');
            }
            "-f" if i + 1 < args.len() => {
                fields = parse_field_spec(args[i + 1]);
                i += 1;
            }
            s if s.starts_with("-f") => {
                fields = parse_field_spec(&s[2..]);
            }
            _ => {}
        }
        i += 1;
    }

    if fields.is_empty() {
        return ShellOutput::text("usage: cut -d<delim> -f<fields> [file]\r\n");
    }

    let input = match get_input(args, piped, 0).await {
        Some(s) => s,
        None => return ShellOutput::text(""),
    };

    let mut out = String::new();
    for line in input.lines() {
        let parts: Vec<&str> = line.split(delimiter).collect();
        let selected: Vec<&str> = fields
            .iter()
            .filter_map(|&f| parts.get(f.saturating_sub(1)).copied())
            .collect();
        out.push_str(&selected.join(&delimiter.to_string()));
        out.push_str("\r\n");
    }
    ShellOutput::text(out)
}

fn parse_field_spec(spec: &str) -> Vec<usize> {
    let mut fields = Vec::new();
    for part in spec.split(',') {
        if let Some(pos) = part.find('-') {
            let start: usize = part[..pos].parse().unwrap_or(1);
            let end: usize = part[pos + 1..].parse().unwrap_or(start);
            for f in start..=end {
                fields.push(f);
            }
        } else if let Ok(f) = part.parse() {
            fields.push(f);
        }
    }
    fields
}

pub async fn cmd_tr(args: &[&str], piped: Option<&str>) -> ShellOutput {
    if args.len() < 2 {
        return ShellOutput::text("usage: tr <from> <to>\r\n");
    }

    let input = match piped {
        Some(s) => s.replace("\r\n", "\n"),
        None => return ShellOutput::text("usage: <cmd> | tr <from> <to>\r\n"),
    };

    let from_chars: Vec<char> = args[0].chars().collect();
    let to_chars: Vec<char> = args[1].chars().collect();

    let mut out = String::new();
    for c in input.chars() {
        if let Some(pos) = from_chars.iter().position(|&fc| fc == c) {
            if let Some(&tc) = to_chars.get(pos) {
                out.push(tc);
            } else if let Some(&last) = to_chars.last() {
                out.push(last);
            }
        } else {
            out.push(c);
        }
    }

    ShellOutput::text(out.replace('\n', "\r\n"))
}

pub async fn cmd_sed(args: &[&str], piped: Option<&str>) -> ShellOutput {
    // Support basic s/pattern/replacement/[g] only
    let expr = match args.first() {
        Some(e) => *e,
        None => return ShellOutput::text("usage: sed 's/pattern/replacement/[g]'\r\n"),
    };

    let input = match get_input(args, piped, 1).await {
        Some(s) => s,
        None => return ShellOutput::text("usage: sed 's/pat/rep/' [file]\r\n"),
    };

    // Parse s/pat/rep/flags
    if !expr.starts_with("s") || expr.len() < 4 {
        return ShellOutput::text("sed: only s/pattern/replacement/[g] is supported\r\n");
    }

    let delim = expr.chars().nth(1).unwrap_or('/');
    let rest = &expr[2..];
    let parts: Vec<&str> = rest.splitn(3, delim).collect();
    if parts.len() < 2 {
        return ShellOutput::text("sed: invalid expression\r\n");
    }

    let pattern = parts[0];
    let replacement = parts[1];
    let global = parts.get(2).map(|f| f.contains('g')).unwrap_or(false);

    let mut out = String::new();
    for line in input.lines() {
        let new_line = if global {
            line.replace(pattern, replacement)
        } else {
            line.replacen(pattern, replacement, 1)
        };
        out.push_str(&new_line);
        out.push_str("\r\n");
    }
    ShellOutput::text(out)
}

pub fn cmd_rev(piped: Option<&str>) -> ShellOutput {
    let input = match piped {
        Some(s) => s.replace("\r\n", "\n"),
        None => return ShellOutput::text("usage: <cmd> | rev\r\n"),
    };

    let mut out = String::new();
    for line in input.lines() {
        let reversed: String = line.chars().rev().collect();
        out.push_str(&reversed);
        out.push_str("\r\n");
    }
    ShellOutput::text(out)
}

pub fn cmd_base64(args: &[&str], piped: Option<&str>) -> ShellOutput {
    let decode = args.iter().any(|a| *a == "-d" || *a == "--decode");

    let input = match piped {
        Some(s) => s.replace("\r\n", "\n"),
        None => return ShellOutput::text("usage: <cmd> | base64 [-d]\r\n"),
    };

    if decode {
        let clean: String = input.chars().filter(|c| !c.is_whitespace()).collect();
        match base64_decode(&clean) {
            Ok(data) => {
                let text = String::from_utf8_lossy(&data);
                ShellOutput::text(text.replace('\n', "\r\n"))
            }
            Err(e) => ShellOutput::text(format!("base64: {}\r\n", e)),
        }
    } else {
        let encoded = base64_encode(input.trim_end().as_bytes());
        ShellOutput::text(format!("{}\r\n", encoded))
    }
}

const B64_TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(B64_TABLE[((triple >> 18) & 0x3F) as usize] as char);
        result.push(B64_TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(B64_TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(B64_TABLE[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn base64_decode(s: &str) -> Result<Vec<u8>, &'static str> {
    let mut result = Vec::new();
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        let val = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return Err("invalid base64 character"),
        } as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            result.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(result)
}

pub fn cmd_xxd(piped: Option<&str>) -> ShellOutput {
    let input = match piped {
        Some(s) => s,
        None => return ShellOutput::text("usage: <cmd> | xxd\r\n"),
    };

    let bytes = input.as_bytes();
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}: ", i * 16));
        for (j, &b) in chunk.iter().enumerate() {
            out.push_str(&format!("{:02x}", b));
            if j % 2 == 1 {
                out.push(' ');
            }
        }
        // Pad if short
        let hex_width = 40; // 16 bytes * 2 hex + 8 spaces
        let current = chunk.len() * 2 + chunk.len() / 2;
        for _ in current..hex_width {
            out.push(' ');
        }
        out.push(' ');
        for &b in chunk {
            if b >= 0x20 && b < 0x7f {
                out.push(b as char);
            } else {
                out.push('.');
            }
        }
        out.push_str("\r\n");
    }
    ShellOutput::text(out)
}

pub fn cmd_xargs<'a>(
    args: &'a [&'a str],
    piped: Option<&'a str>,
    state: &'a std::sync::Arc<crate::api::AppState>,
    container_name: &'a str,
    started_at: chrono::DateTime<chrono::Utc>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ShellOutput> + Send + 'a>> {
    let input = match piped {
        Some(s) => s.replace("\r\n", "\n"),
        None => return Box::pin(async { ShellOutput::text("usage: <cmd> | xargs <command>\r\n") }),
    };

    if args.is_empty() {
        return Box::pin(async { ShellOutput::text("usage: <cmd> | xargs <command>\r\n") });
    }

    let cmd = args.join(" ");
    let state = state.clone();
    let container_name = container_name.to_string();

    Box::pin(async move {
        let mut out = String::new();
        for line in input.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let full_cmd = format!("{} {}", cmd, line);
            let result =
                super::execute_single(&full_cmd, &state, &container_name, started_at, None).await;
            out.push_str(&result.text);
        }
        ShellOutput::text(out)
    })
}
