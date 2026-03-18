use super::ShellOutput;
use std::path::Path;

pub async fn cmd_ls(args: &[&str]) -> ShellOutput {
    let mut long = false;
    let mut all = false;
    let mut path_arg = None;

    for arg in args {
        match *arg {
            "-l" => long = true,
            "-a" => all = true,
            "-la" | "-al" | "-lah" | "-alh" => {
                long = true;
                all = true;
            }
            "-h" => {}
            _ if !arg.starts_with('-') => path_arg = Some(*arg),
            _ => {}
        }
    }

    let path = path_arg.unwrap_or(".");
    let dir_path = Path::new(path);

    // If path is a file, just show info about it
    match tokio::fs::metadata(dir_path).await {
        Ok(meta) if !meta.is_dir() => {
            if long {
                return ShellOutput::text(format!(
                    "{}\r\n",
                    format_file_long(path, &meta)
                ));
            } else {
                return ShellOutput::text(format!("{}\r\n", path));
            }
        }
        Err(e) => return ShellOutput::text(format!("ls: {}: {}\r\n", path, e)),
        _ => {}
    }

    let mut entries = match tokio::fs::read_dir(dir_path).await {
        Ok(e) => e,
        Err(e) => return ShellOutput::text(format!("ls: {}: {}\r\n", path, e)),
    };

    let mut items = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !all && name.starts_with('.') {
            continue;
        }
        let meta = entry.metadata().await.ok();
        items.push((name, meta, entry.file_type().await.ok()));
    }
    items.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    let mut out = String::new();
    if long {
        for (name, meta, ft) in &items {
            if let Some(m) = meta {
                out.push_str(&format_file_long(&name, m));
            } else {
                let indicator = match ft {
                    Some(t) if t.is_dir() => "/",
                    Some(t) if t.is_symlink() => "@",
                    _ => "",
                };
                out.push_str(&format!("?????????? ? ? ? ? {}{}", name, indicator));
            }
            out.push_str("\r\n");
        }
    } else {
        let mut line = String::new();
        let mut col = 0;
        for (name, _meta, ft) in &items {
            let indicator = match ft {
                Some(t) if t.is_dir() => "/",
                Some(t) if t.is_symlink() => "@",
                _ => "",
            };
            let display = format!("{}{}", name, indicator);
            let width = display.len() + 2;
            if col + width > 80 && col > 0 {
                out.push_str(line.trim_end());
                out.push_str("\r\n");
                line.clear();
                col = 0;
            }
            let colored = match ft {
                Some(t) if t.is_dir() => format!("\x1b[1;34m{}\x1b[0m  ", display),
                Some(t) if t.is_symlink() => format!("\x1b[1;36m{}\x1b[0m  ", display),
                _ => format!("{}  ", display),
            };
            line.push_str(&colored);
            col += width;
        }
        if !line.is_empty() {
            out.push_str(line.trim_end());
            out.push_str("\r\n");
        }
    }

    if out.is_empty() {
        out.push_str("\r\n");
    }
    ShellOutput::text(out)
}

fn format_file_long(name: &str, meta: &std::fs::Metadata) -> String {
    let size = meta.len();
    let is_dir = meta.is_dir();
    let is_symlink = meta.file_type().is_symlink();

    let perms = format_permissions(meta, is_dir, is_symlink);
    let size_str = format_size_human(size);

    let modified = {
        use std::time::SystemTime;
        meta.modified()
            .unwrap_or(SystemTime::UNIX_EPOCH)
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| {
                let secs = d.as_secs() as i64;
                let dt = chrono::DateTime::from_timestamp(secs, 0)
                    .unwrap_or_default();
                dt.format("%b %e %H:%M").to_string()
            })
            .unwrap_or_else(|_| "???".into())
    };

    let indicator = if is_dir {
        "/"
    } else if is_symlink {
        "@"
    } else {
        ""
    };

    let colored_name = if is_dir {
        format!("\x1b[1;34m{}/\x1b[0m", name)
    } else if is_symlink {
        format!("\x1b[1;36m{}@\x1b[0m", name)
    } else if is_executable(meta) {
        format!("\x1b[1;32m{}\x1b[0m", name)
    } else {
        let _ = indicator;
        name.to_string()
    };

    format!(
        "{} {:>8} {} {}",
        perms, size_str, modified, colored_name
    )
}

fn format_permissions(meta: &std::fs::Metadata, is_dir: bool, is_symlink: bool) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        let file_type = if is_symlink {
            'l'
        } else if is_dir {
            'd'
        } else {
            '-'
        };
        let perms = [
            if mode & 0o400 != 0 { 'r' } else { '-' },
            if mode & 0o200 != 0 { 'w' } else { '-' },
            if mode & 0o100 != 0 { 'x' } else { '-' },
            if mode & 0o040 != 0 { 'r' } else { '-' },
            if mode & 0o020 != 0 { 'w' } else { '-' },
            if mode & 0o010 != 0 { 'x' } else { '-' },
            if mode & 0o004 != 0 { 'r' } else { '-' },
            if mode & 0o002 != 0 { 'w' } else { '-' },
            if mode & 0o001 != 0 { 'x' } else { '-' },
        ];
        format!(
            "{}{}",
            file_type,
            perms.iter().collect::<String>()
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (meta, is_symlink);
        if is_dir {
            "d---------".to_string()
        } else {
            "----------".to_string()
        }
    }
}

fn is_executable(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

pub fn format_size_human(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{}", bytes);
    }
    let units = ["K", "M", "G", "T"];
    let mut val = bytes as f64 / 1024.0;
    for unit in &units {
        if val < 1024.0 {
            return format!("{:.1}{}", val, unit);
        }
        val /= 1024.0;
    }
    format!("{:.1}P", val)
}

pub async fn cmd_cat(args: &[&str], piped: Option<&str>) -> ShellOutput {
    if args.is_empty() {
        if let Some(input) = piped {
            return ShellOutput::text(input.replace('\n', "\r\n"));
        }
        return ShellOutput::text("usage: cat <file> [...]\r\n");
    }

    let mut out = String::new();
    for path in args {
        if path.starts_with('-') {
            continue;
        }
        match tokio::fs::read_to_string(path).await {
            Ok(content) => out.push_str(&content.replace('\n', "\r\n")),
            Err(e) => out.push_str(&format!("cat: {}: {}\r\n", path, e)),
        }
    }
    if !out.ends_with("\r\n") && !out.is_empty() {
        out.push_str("\r\n");
    }
    ShellOutput::text(out)
}

pub async fn cmd_head(args: &[&str], piped: Option<&str>) -> ShellOutput {
    let mut count = 10usize;
    let mut file = None;

    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-n" if i + 1 < args.len() => {
                count = args[i + 1].parse().unwrap_or(10);
                i += 1;
            }
            s if s.starts_with('-') && s[1..].parse::<usize>().is_ok() => {
                count = s[1..].parse().unwrap_or(10);
            }
            _ if !args[i].starts_with('-') => file = Some(args[i]),
            _ => {}
        }
        i += 1;
    }

    let content = if let Some(path) = file {
        match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => return ShellOutput::text(format!("head: {}: {}\r\n", path, e)),
        }
    } else if let Some(input) = piped {
        input.replace("\r\n", "\n")
    } else {
        return ShellOutput::text("usage: head [-n N] <file>\r\n");
    };

    let lines: Vec<&str> = content.lines().take(count).collect();
    ShellOutput::text(lines.join("\r\n") + "\r\n")
}

pub async fn cmd_tail(args: &[&str], piped: Option<&str>) -> ShellOutput {
    let mut count = 10usize;
    let mut file = None;

    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-n" if i + 1 < args.len() => {
                count = args[i + 1].parse().unwrap_or(10);
                i += 1;
            }
            s if s.starts_with('-') && s[1..].parse::<usize>().is_ok() => {
                count = s[1..].parse().unwrap_or(10);
            }
            _ if !args[i].starts_with('-') => file = Some(args[i]),
            _ => {}
        }
        i += 1;
    }

    let content = if let Some(path) = file {
        match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => return ShellOutput::text(format!("tail: {}: {}\r\n", path, e)),
        }
    } else if let Some(input) = piped {
        input.replace("\r\n", "\n")
    } else {
        return ShellOutput::text("usage: tail [-n N] <file>\r\n");
    };

    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(count);
    ShellOutput::text(lines[start..].join("\r\n") + "\r\n")
}

pub async fn cmd_cp(args: &[&str]) -> ShellOutput {
    if args.len() < 2 {
        return ShellOutput::text("usage: cp <src> <dst>\r\n");
    }
    let src = args[args.len() - 2];
    let dst = args[args.len() - 1];
    match tokio::fs::copy(src, dst).await {
        Ok(bytes) => ShellOutput::text(format!("copied {} bytes\r\n", bytes)),
        Err(e) => ShellOutput::text(format!("cp: {}\r\n", e)),
    }
}

pub async fn cmd_mv(args: &[&str]) -> ShellOutput {
    if args.len() < 2 {
        return ShellOutput::text("usage: mv <src> <dst>\r\n");
    }
    let src = args[args.len() - 2];
    let dst = args[args.len() - 1];
    match tokio::fs::rename(src, dst).await {
        Ok(()) => ShellOutput::text(""),
        Err(e) => ShellOutput::text(format!("mv: {}\r\n", e)),
    }
}

pub async fn cmd_rm(args: &[&str]) -> ShellOutput {
    let mut recursive = false;
    let mut force = false;
    let mut targets = Vec::new();

    for arg in args {
        match *arg {
            "-r" | "-R" => recursive = true,
            "-f" => force = true,
            "-rf" | "-fr" => {
                recursive = true;
                force = true;
            }
            _ if !arg.starts_with('-') => targets.push(*arg),
            _ => {}
        }
    }

    if targets.is_empty() {
        return ShellOutput::text("usage: rm [-rf] <path> [...]\r\n");
    }

    let mut out = String::new();
    for target in targets {
        let meta = match tokio::fs::metadata(target).await {
            Ok(m) => m,
            Err(e) => {
                if !force {
                    out.push_str(&format!("rm: {}: {}\r\n", target, e));
                }
                continue;
            }
        };

        let result = if meta.is_dir() {
            if recursive {
                tokio::fs::remove_dir_all(target).await
            } else {
                out.push_str(&format!("rm: {}: is a directory\r\n", target));
                continue;
            }
        } else {
            tokio::fs::remove_file(target).await
        };

        if let Err(e) = result {
            out.push_str(&format!("rm: {}: {}\r\n", target, e));
        }
    }
    ShellOutput::text(out)
}

pub async fn cmd_mkdir(args: &[&str]) -> ShellOutput {
    let mut parents = false;
    let mut dirs = Vec::new();

    for arg in args {
        match *arg {
            "-p" => parents = true,
            _ if !arg.starts_with('-') => dirs.push(*arg),
            _ => {}
        }
    }

    if dirs.is_empty() {
        return ShellOutput::text("usage: mkdir [-p] <dir> [...]\r\n");
    }

    let mut out = String::new();
    for dir in dirs {
        let result = if parents {
            tokio::fs::create_dir_all(dir).await
        } else {
            tokio::fs::create_dir(dir).await
        };
        if let Err(e) = result {
            out.push_str(&format!("mkdir: {}: {}\r\n", dir, e));
        }
    }
    ShellOutput::text(out)
}

pub async fn cmd_touch(args: &[&str]) -> ShellOutput {
    if args.is_empty() {
        return ShellOutput::text("usage: touch <file> [...]\r\n");
    }

    let mut out = String::new();
    for path in args {
        if path.starts_with('-') {
            continue;
        }
        if Path::new(path).exists() {
            // Update mtime by opening and closing
            if let Err(e) = tokio::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .await
            {
                out.push_str(&format!("touch: {}: {}\r\n", path, e));
            }
        } else if let Err(e) = tokio::fs::write(path, b"").await {
            out.push_str(&format!("touch: {}: {}\r\n", path, e));
        }
    }
    ShellOutput::text(out)
}

pub async fn cmd_chmod(args: &[&str]) -> ShellOutput {
    if args.len() < 2 {
        return ShellOutput::text("usage: chmod <mode> <file> [...]\r\n");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode_str = args[0];
        let mode = match u32::from_str_radix(mode_str, 8) {
            Ok(m) => m,
            Err(_) => return ShellOutput::text(format!("chmod: invalid mode '{}'\r\n", mode_str)),
        };

        let mut out = String::new();
        for path in &args[1..] {
            let perms = std::fs::Permissions::from_mode(mode);
            if let Err(e) = std::fs::set_permissions(path, perms) {
                out.push_str(&format!("chmod: {}: {}\r\n", path, e));
            }
        }
        ShellOutput::text(out)
    }

    #[cfg(not(unix))]
    {
        let _ = args;
        ShellOutput::text("chmod: not available on this platform\r\n")
    }
}

pub async fn cmd_chown(args: &[&str]) -> ShellOutput {
    #[cfg(target_os = "linux")]
    {
        if args.len() < 2 {
            return ShellOutput::text("usage: chown <user[:group]> <file> [...]\r\n");
        }
        let spec = args[0];
        let (uid_str, gid_str) = if let Some(pos) = spec.find(':') {
            (&spec[..pos], Some(&spec[pos + 1..]))
        } else {
            (spec, None)
        };

        let uid: u32 = match uid_str.parse() {
            Ok(u) => u,
            Err(_) => return ShellOutput::text(format!("chown: invalid user '{}'\r\n", uid_str)),
        };
        let gid: Option<u32> = gid_str.and_then(|g| g.parse().ok());

        let mut out = String::new();
        for path in &args[1..] {
            let result = nix::unistd::chown(
                *path,
                Some(nix::unistd::Uid::from_raw(uid)),
                gid.map(nix::unistd::Gid::from_raw),
            );
            if let Err(e) = result {
                out.push_str(&format!("chown: {}: {}\r\n", path, e));
            }
        }
        ShellOutput::text(out)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = args;
        ShellOutput::text("chown: not available on this platform\r\n")
    }
}

pub async fn cmd_find(args: &[&str]) -> ShellOutput {
    let mut path = ".";
    let mut name_pattern = None;
    let mut file_type = None;

    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-name" if i + 1 < args.len() => {
                name_pattern = Some(args[i + 1]);
                i += 1;
            }
            "-type" if i + 1 < args.len() => {
                file_type = Some(args[i + 1]);
                i += 1;
            }
            _ if !args[i].starts_with('-') && name_pattern.is_none() => {
                path = args[i];
            }
            _ => {}
        }
        i += 1;
    }

    let mut results = Vec::new();
    find_recursive(Path::new(path), name_pattern, file_type, &mut results, 0).await;
    results.sort();

    if results.is_empty() {
        ShellOutput::text("")
    } else {
        ShellOutput::text(results.join("\r\n") + "\r\n")
    }
}

async fn find_recursive(
    dir: &Path,
    pattern: Option<&str>,
    file_type: Option<&str>,
    results: &mut Vec<String>,
    depth: usize,
) {
    if depth > 20 {
        return;
    }

    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let ft = entry.file_type().await.ok();

        let type_match = match (file_type, &ft) {
            (Some("f"), Some(t)) => t.is_file(),
            (Some("d"), Some(t)) => t.is_dir(),
            (Some("l"), Some(t)) => t.is_symlink(),
            (None, _) => true,
            _ => false,
        };

        let name_match = match pattern {
            Some(pat) => glob_match(pat, &name),
            None => true,
        };

        if type_match && name_match {
            results.push(path.to_string_lossy().to_string());
        }

        if ft.map(|t| t.is_dir()).unwrap_or(false) {
            Box::pin(find_recursive(&path, pattern, file_type, results, depth + 1)).await;
        }

        if results.len() > 10000 {
            return;
        }
    }
}

fn glob_match(pattern: &str, name: &str) -> bool {
    let mut pi = pattern.chars().peekable();
    let mut ni = name.chars().peekable();

    while let Some(&pc) = pi.peek() {
        match pc {
            '*' => {
                pi.next();
                if pi.peek().is_none() {
                    return true;
                }
                while ni.peek().is_some() {
                    let remaining_pattern: String = pi.clone().collect();
                    let remaining_name: String = ni.clone().collect();
                    if glob_match(&remaining_pattern, &remaining_name) {
                        return true;
                    }
                    ni.next();
                }
                return false;
            }
            '?' => {
                pi.next();
                if ni.next().is_none() {
                    return false;
                }
            }
            c => {
                pi.next();
                match ni.next() {
                    Some(nc) if nc == c => {}
                    _ => return false,
                }
            }
        }
    }

    ni.peek().is_none()
}

pub async fn cmd_ln(args: &[&str]) -> ShellOutput {
    let mut symbolic = false;
    let mut targets = Vec::new();

    for arg in args {
        match *arg {
            "-s" => symbolic = true,
            _ if !arg.starts_with('-') => targets.push(*arg),
            _ => {}
        }
    }

    if targets.len() < 2 {
        return ShellOutput::text("usage: ln [-s] <target> <link>\r\n");
    }

    let target = targets[0];
    let link = targets[1];

    #[cfg(unix)]
    {
        let result = if symbolic {
            std::os::unix::fs::symlink(target, link)
        } else {
            std::fs::hard_link(target, link)
        };
        match result {
            Ok(()) => ShellOutput::text(""),
            Err(e) => ShellOutput::text(format!("ln: {}\r\n", e)),
        }
    }

    #[cfg(not(unix))]
    {
        let _ = (target, link, symbolic);
        ShellOutput::text("ln: not available on this platform\r\n")
    }
}

pub async fn cmd_stat(args: &[&str]) -> ShellOutput {
    if args.is_empty() {
        return ShellOutput::text("usage: stat <file> [...]\r\n");
    }

    let mut out = String::new();
    for path in args {
        if path.starts_with('-') {
            continue;
        }
        match tokio::fs::metadata(path).await {
            Ok(meta) => {
                out.push_str(&format!("  File: {}\r\n", path));
                out.push_str(&format!(
                    "  Size: {:<15} {}\r\n",
                    meta.len(),
                    if meta.is_dir() {
                        "directory"
                    } else if meta.is_symlink() {
                        "symbolic link"
                    } else {
                        "regular file"
                    }
                ));
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    out.push_str(&format!(
                        "Device: {:<15} Inode: {:<10} Links: {}\r\n",
                        meta.dev(),
                        meta.ino(),
                        meta.nlink()
                    ));
                    out.push_str(&format!(
                        "Access: ({:04o}/{})  Uid: {}  Gid: {}\r\n",
                        meta.mode() & 0o7777,
                        format_permissions(&meta, meta.is_dir(), meta.is_symlink()),
                        meta.uid(),
                        meta.gid()
                    ));
                }
                if let Ok(modified) = meta.modified() {
                    if let Ok(dur) = modified.duration_since(std::time::SystemTime::UNIX_EPOCH) {
                        let dt =
                            chrono::DateTime::from_timestamp(dur.as_secs() as i64, 0)
                                .unwrap_or_default();
                        out.push_str(&format!(
                            "Modify: {}\r\n",
                            dt.format("%Y-%m-%d %H:%M:%S")
                        ));
                    }
                }
                out.push_str("\r\n");
            }
            Err(e) => out.push_str(&format!("stat: {}: {}\r\n", path, e)),
        }
    }
    ShellOutput::text(out)
}

pub fn cmd_pwd() -> ShellOutput {
    match std::env::current_dir() {
        Ok(p) => ShellOutput::text(format!("{}\r\n", p.display())),
        Err(e) => ShellOutput::text(format!("pwd: {}\r\n", e)),
    }
}

pub async fn cmd_wc(args: &[&str], piped: Option<&str>) -> ShellOutput {
    let mut show_lines = false;
    let mut show_words = false;
    let mut show_chars = false;
    let mut files = Vec::new();

    for arg in args {
        match *arg {
            "-l" => show_lines = true,
            "-w" => show_words = true,
            "-c" | "-m" => show_chars = true,
            "-lwc" | "-lw" | "-lc" | "-wc" => {
                if arg.contains('l') {
                    show_lines = true;
                }
                if arg.contains('w') {
                    show_words = true;
                }
                if arg.contains('c') {
                    show_chars = true;
                }
            }
            _ if !arg.starts_with('-') => files.push(*arg),
            _ => {}
        }
    }

    // Default: show all
    if !show_lines && !show_words && !show_chars {
        show_lines = true;
        show_words = true;
        show_chars = true;
    }

    let content = if !files.is_empty() {
        let mut all = String::new();
        for f in &files {
            match tokio::fs::read_to_string(f).await {
                Ok(c) => all.push_str(&c),
                Err(e) => return ShellOutput::text(format!("wc: {}: {}\r\n", f, e)),
            }
        }
        all
    } else if let Some(input) = piped {
        input.replace("\r\n", "\n")
    } else {
        return ShellOutput::text("usage: wc [-lwc] <file>\r\n");
    };

    let lines = content.lines().count();
    let words = content.split_whitespace().count();
    let chars = content.len();

    let mut parts = Vec::new();
    if show_lines {
        parts.push(format!("{:>8}", lines));
    }
    if show_words {
        parts.push(format!("{:>8}", words));
    }
    if show_chars {
        parts.push(format!("{:>8}", chars));
    }

    let label = if files.len() == 1 {
        format!(" {}", files[0])
    } else {
        String::new()
    };
    ShellOutput::text(format!("{}{}\r\n", parts.join(""), label))
}

pub async fn cmd_du(args: &[&str]) -> ShellOutput {
    let mut human = false;
    let mut summary = false;
    let mut path = ".";

    for arg in args {
        match *arg {
            "-h" => human = true,
            "-s" => summary = true,
            "-sh" | "-hs" => {
                summary = true;
                human = true;
            }
            _ if !arg.starts_with('-') => path = arg,
            _ => {}
        }
    }

    if summary {
        let size = dir_size(Path::new(path)).await;
        let display = if human {
            format_size_human(size)
        } else {
            format!("{}", size / 1024)
        };
        return ShellOutput::text(format!("{}\t{}\r\n", display, path));
    }

    // Show each directory
    let mut out = String::new();
    du_recursive(Path::new(path), human, &mut out, 0).await;
    ShellOutput::text(out)
}

async fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut entries = match tokio::fs::read_dir(path).await {
        Ok(e) => e,
        Err(_) => return 0,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let p = entry.path();
        if let Ok(meta) = tokio::fs::symlink_metadata(&p).await {
            if meta.is_dir() {
                total += Box::pin(dir_size(&p)).await;
            } else {
                total += meta.len();
            }
        }
    }
    total
}

async fn du_recursive(path: &Path, human: bool, out: &mut String, depth: usize) {
    if depth > 10 {
        return;
    }
    let size = dir_size(path).await;
    let display = if human {
        format_size_human(size)
    } else {
        format!("{}", size / 1024)
    };
    out.push_str(&format!("{}\t{}\r\n", display, path.display()));

    let mut entries = match tokio::fs::read_dir(path).await {
        Ok(e) => e,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(ft) = entry.file_type().await {
            if ft.is_dir() {
                Box::pin(du_recursive(&entry.path(), human, out, depth + 1)).await;
            }
        }
    }
}

pub async fn cmd_readlink(args: &[&str]) -> ShellOutput {
    if args.is_empty() {
        return ShellOutput::text("usage: readlink <path>\r\n");
    }
    let path = args[0];
    match tokio::fs::read_link(path).await {
        Ok(target) => ShellOutput::text(format!("{}\r\n", target.display())),
        Err(e) => ShellOutput::text(format!("readlink: {}: {}\r\n", path, e)),
    }
}

pub async fn cmd_file_type(args: &[&str]) -> ShellOutput {
    if args.is_empty() {
        return ShellOutput::text("usage: file <path> [...]\r\n");
    }

    let mut out = String::new();
    for path in args {
        if path.starts_with('-') {
            continue;
        }
        let meta = match tokio::fs::symlink_metadata(path).await {
            Ok(m) => m,
            Err(e) => {
                out.push_str(&format!("{}: cannot open ({})\r\n", path, e));
                continue;
            }
        };

        if meta.is_dir() {
            out.push_str(&format!("{}: directory\r\n", path));
            continue;
        }
        if meta.file_type().is_symlink() {
            match tokio::fs::read_link(path).await {
                Ok(target) => {
                    out.push_str(&format!(
                        "{}: symbolic link to {}\r\n",
                        path,
                        target.display()
                    ));
                }
                Err(_) => out.push_str(&format!("{}: symbolic link\r\n", path)),
            }
            continue;
        }

        // Read first bytes to detect type
        match tokio::fs::read(path).await {
            Ok(data) => {
                let desc = detect_file_type(&data);
                out.push_str(&format!("{}: {}\r\n", path, desc));
            }
            Err(e) => out.push_str(&format!("{}: cannot read ({})\r\n", path, e)),
        }
    }
    ShellOutput::text(out)
}

fn detect_file_type(data: &[u8]) -> &'static str {
    if data.len() < 4 {
        return "empty or very small file";
    }
    // ELF
    if data.starts_with(b"\x7fELF") {
        return "ELF executable";
    }
    // Gzip
    if data.starts_with(b"\x1f\x8b") {
        return "gzip compressed data";
    }
    // Tar
    if data.len() > 262 && &data[257..262] == b"ustar" {
        return "tar archive";
    }
    // PNG
    if data.starts_with(b"\x89PNG") {
        return "PNG image";
    }
    // JPEG
    if data.starts_with(b"\xff\xd8\xff") {
        return "JPEG image";
    }
    // PDF
    if data.starts_with(b"%PDF") {
        return "PDF document";
    }
    // Zip
    if data.starts_with(b"PK\x03\x04") {
        return "Zip archive";
    }
    // Shell script
    if data.starts_with(b"#!") {
        return "script, text executable";
    }
    // Check if it's text
    let sample = &data[..data.len().min(512)];
    if sample.iter().all(|&b| b == b'\n' || b == b'\r' || b == b'\t' || (b >= 0x20 && b < 0x7f)) {
        return "ASCII text";
    }
    if sample
        .iter()
        .filter(|&&b| b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t')
        .count()
        < sample.len() / 10
    {
        return "UTF-8 Unicode text";
    }
    "data"
}

pub async fn cmd_sha256sum(args: &[&str], piped: Option<&str>) -> ShellOutput {
    use sha2::{Digest, Sha256};

    if args.is_empty() && piped.is_none() {
        return ShellOutput::text("usage: sha256sum <file> [...]\r\n");
    }

    if args.is_empty() {
        if let Some(input) = piped {
            let hash = Sha256::digest(input.as_bytes());
            return ShellOutput::text(format!("{}  -\r\n", hex::encode(hash)));
        }
    }

    let mut out = String::new();
    for path in args {
        if path.starts_with('-') {
            continue;
        }
        match tokio::fs::read(path).await {
            Ok(data) => {
                let hash = Sha256::digest(&data);
                out.push_str(&format!("{}  {}\r\n", hex::encode(hash), path));
            }
            Err(e) => out.push_str(&format!("sha256sum: {}: {}\r\n", path, e)),
        }
    }
    ShellOutput::text(out)
}

pub async fn cmd_tee(args: &[&str], piped: Option<&str>) -> ShellOutput {
    let input = match piped {
        Some(s) => s,
        None => return ShellOutput::text("usage: <cmd> | tee <file>\r\n"),
    };

    let mut append = false;
    let mut file = None;
    for arg in args {
        match *arg {
            "-a" => append = true,
            _ if !arg.starts_with('-') => file = Some(*arg),
            _ => {}
        }
    }

    if let Some(path) = file {
        let content = input.replace("\r\n", "\n");
        let result = if append {
            use tokio::io::AsyncWriteExt;
            match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
            {
                Ok(mut f) => f.write_all(content.as_bytes()).await,
                Err(e) => Err(e),
            }
        } else {
            tokio::fs::write(path, content.as_bytes()).await
        };

        if let Err(e) = result {
            return ShellOutput::text(format!("tee: {}: {}\r\n", path, e));
        }
    }

    // Pass through input to stdout
    ShellOutput::text(input.to_string())
}
