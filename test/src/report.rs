//! Results as the test standard wants them: one JSON object per test on
//! stdout, a summary line last, the same lines under `/results`, and an exit
//! code of 0 (all passed), 1 (a test failed) or 2 (could not run).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::time::Instant;

/// How one test came out.
pub enum Outcome {
    Pass(String),
    Fail(String),
    /// Not applicable here (no stormd of the node's own answered); never
    /// counted as a pass.
    Skip(String),
    /// The test could not run (the stormd binary is missing, no port can be
    /// bound, a variable the runner must set is wrong). Reported as a
    /// failure, exits 2.
    Infra(String),
}

pub struct Report {
    pass: u32,
    fail: u32,
    skip: u32,
    infra: u32,
    file: Option<File>,
}

impl Report {
    pub fn new(results: &std::path::Path) -> Report {
        // `/results` is the runner's volume; without it (a hand run) the
        // lines still go to stdout.
        let file = OpenOptions::new().create(true).append(true).open(results.join("results.jsonl")).ok();
        Report { pass: 0, fail: 0, skip: 0, infra: 0, file }
    }

    /// Run one test, timed, and record it.
    pub fn run(&mut self, name: &str, f: impl FnOnce() -> Outcome) -> bool {
        let t = Instant::now();
        let outcome = f();
        let ms = t.elapsed().as_millis();
        self.record(name, outcome, ms, None)
    }

    /// Record a result; `extra` is a JSON object's members, added verbatim.
    /// Returns whether it passed.
    pub fn record(&mut self, name: &str, outcome: Outcome, ms: u128, extra: Option<&str>) -> bool {
        let (status, detail) = match outcome {
            Outcome::Pass(d) => {
                self.pass += 1;
                ("pass", d)
            }
            Outcome::Fail(d) => {
                self.fail += 1;
                ("fail", d)
            }
            Outcome::Skip(d) => {
                self.skip += 1;
                ("skip", d)
            }
            Outcome::Infra(d) => {
                self.infra += 1;
                ("fail", format!("could not run: {d}"))
            }
        };
        let extra = extra.map(|e| format!(", {e}")).unwrap_or_default();
        self.line(&format!(
            "{{\"test\": {}, \"status\": \"{status}\", \"ms\": {ms}, \"detail\": {}{extra}}}",
            json(name),
            json(&detail)
        ));
        status == "pass"
    }

    fn line(&mut self, l: &str) {
        println!("{l}");
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "{l}");
        }
    }

    /// Print the summary and return the exit code. A real failure outranks
    /// an infrastructure one: it is the more useful thing to be told.
    pub fn finish(mut self) -> i32 {
        let fail = self.fail + self.infra;
        let s = format!(
            "{{\"summary\": {{\"pass\": {}, \"fail\": {fail}, \"skip\": {}}}}}",
            self.pass, self.skip
        );
        self.line(&s);
        match (self.fail, self.infra) {
            (0, 0) => 0,
            (0, _) => 2,
            _ => 1,
        }
    }
}

/// A JSON string literal.
pub fn json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn json_strings_escape() {
        assert_eq!(super::json("a\"b\\c\nd\u{1b}"), r#""a\"b\\c\nd\u001b""#);
        assert_eq!(super::json("em — dash"), "\"em — dash\"");
    }
}
