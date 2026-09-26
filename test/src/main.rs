//! stormd's test container (stormcentral `docs/test-standard.md`).
//!
//! stormd is the init of every supervised container on a node: what it does
//! there is start processes in order, keep them up, report them through its
//! REST API, and stop them when told. So the suites run **the stormd of the
//! commit under test** — `/stormd` in the image — as a child of this binary,
//! on configs they write, and check that through the API, the way anything on
//! a node talks to it. The processes it supervises are this binary again, in
//! `helper` mode (the image is `FROM scratch`: there is no shell).
//!
//! - `short`: up, API answering, start order and a ready probe honoured, a
//!   crash restarted, output in the logs, SIGTERM stops it with nothing left
//!   behind — and the node's own stormds answer health, if any are reachable.
//! - `medium`: the failure paths and features, end to end.
//! - `long`: overnight waves of processes sized from this pod's allowance,
//!   measured for slowdown and residue across waves.
//!
//! Nothing is created in the cluster; everything lives in the pod, so cleanup
//! is the pod going away, and every instance kills what it started on drop.

mod env;
mod harness;
mod helper;
mod long;
mod medium;
mod report;
mod short;

use report::{Outcome, Report};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("helper") {
        std::process::exit(helper::run(&args[1..]));
    }

    let env = env::Env::read();
    let mut r = Report::new(&env.results);
    if let Err(e) = std::fs::create_dir_all(&env.work) {
        r.record("setup", Outcome::Infra(format!("cannot create {}: {e}", env.work.display())), 0, None);
        std::process::exit(r.finish());
    }
    if !env.stormd.exists() {
        r.record("setup", Outcome::Infra(format!("no stormd binary at {}", env.stormd.display())), 0, None);
        std::process::exit(r.finish());
    }
    match env.suite.as_str() {
        "short" => short::run(&env, &mut r),
        "medium" => medium::run(&env, &mut r),
        "long" => long::run(&env, &mut r),
        other => {
            r.record("suite", Outcome::Infra(format!("STORM_SUITE {other:?} is not short, medium or long")), 0, None);
        }
    }
    // A hand run's scratch is a temp directory; the image's is the results
    // volume, kept for the runner to collect.
    if !env.work.starts_with(&env.results) {
        let _ = std::fs::remove_dir_all(&env.work);
    }
    std::process::exit(r.finish());
}
