//! `stormd-test helper <mode> …` — the processes the suites give stormd to
//! supervise. The image is `FROM scratch`, so there is no shell to write them
//! in; the test binary plays every part, and each part does one thing whose
//! effect a test can see.
//!
//! Every helper that is still running writes its pid into `$HELPER_PIDS` (a
//! directory) when that is set, so a test can check that nothing stormd
//! started outlives it.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::Duration;

pub fn run(args: &[String]) -> i32 {
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    match arg(0) {
        // Stay up (for `secs`, or forever with none).
        "sleep" => {
            record_pid();
            match arg(1).parse::<u64>() {
                Ok(s) => std::thread::sleep(Duration::from_secs(s)),
                Err(_) => forever(),
            }
            0
        }
        // Exit with `code`, after `ms`.
        "exit" => {
            std::thread::sleep(Duration::from_millis(arg(2).parse().unwrap_or(0)));
            arg(1).parse().unwrap_or(1)
        }
        // Print a marker line (stdout and stderr), then stay up.
        "say" => {
            println!("{} stdout", arg(1));
            eprintln!("{} stderr", arg(1));
            let _ = std::io::stdout().flush();
            record_pid();
            forever();
        }
        // A one-shot's work: wait `ms`, create `path`, exit 0.
        "touch-after" => {
            std::thread::sleep(Duration::from_millis(arg(1).parse().unwrap_or(0)));
            match std::fs::write(arg(2), b"done\n") {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("cannot write {}: {e}", arg(2));
                    1
                }
            }
        }
        // Succeed only if `path` exists — what a dependent of a one-shot does.
        "require" => {
            if Path::new(arg(1)).exists() {
                println!("found {}", arg(1));
                0
            } else {
                eprintln!("missing {}", arg(1));
                1
            }
        }
        // Fail the first run (marking `path`), then stay up.
        "crash-once" => {
            if !Path::new(arg(1)).exists() {
                let _ = std::fs::write(arg(1), b"crashed\n");
                eprintln!("crashing once");
                return 1;
            }
            record_pid();
            forever();
        }
        // Succeed only if something listens on `port` — a dependent that
        // needs its dependency answering, not merely forked.
        "connect" => match std::net::TcpStream::connect(("127.0.0.1", arg(1).parse::<u16>().unwrap_or(0))) {
            Ok(_) => {
                println!("connected to {}", arg(1));
                0
            }
            Err(e) => {
                eprintln!("nothing on {}: {e}", arg(1));
                1
            }
        },
        // Listen on `port` after `ms` (a ready probe's target), and answer
        // HTTP 200 to anything.
        "serve" => {
            std::thread::sleep(Duration::from_millis(arg(2).parse().unwrap_or(0)));
            let l = match TcpListener::bind(("127.0.0.1", arg(1).parse::<u16>().unwrap_or(0))) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("cannot listen on {}: {e}", arg(1));
                    return 1;
                }
            };
            println!("listening on {}", arg(1));
            record_pid();
            for mut c in l.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = c.set_read_timeout(Some(Duration::from_secs(1)));
                let _ = c.read(&mut buf);
                let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            }
            0
        }
        other => {
            eprintln!("unknown helper mode {other:?}");
            64
        }
    }
}

fn record_pid() {
    if let Some(dir) = std::env::var_os("HELPER_PIDS") {
        let _ = std::fs::write(Path::new(&dir).join(std::process::id().to_string()), b"");
    }
}

fn forever() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
