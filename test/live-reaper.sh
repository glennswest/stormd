#!/bin/bash
# Live check for #61 against a built stormd (run from the repo root after
# `cargo build`): stormd as PID 1 of its own PID namespace (unprivileged,
# `unshare -Urpf --mount-proc`), 60 one-shots that finish at once — each
# must be recorded exit 0, not `code=None` — and a process that leaves
# orphans behind, which re-parent to stormd and must be reaped (after the
# 5 s grace). Prints what it saw.
set -u
B=${CARGO_TARGET_DIR:-$PWD/target}/debug/stormd; W=$(mktemp -d); cd $W
{
cat <<C
[general]
log_dir = "$W/log"
[api]
bind = "127.0.0.1:19810"
[ssh]
enabled = false
[stormlog.mcast]
group = "off"
[[process]]
name = "orphans"
command = "/bin/sh"
args = ["-c", "for i in 1 2 3 4 5 6 7 8 9 10; do (sleep 0.5; exit 0) & done; exit 0"]
on_exit = "stop"
C
for i in $(seq 60); do cat <<C
[[process]]
name = "one$i"
command = "/bin/sh"
args = ["-c", "sleep 2; exit 0"]
on_exit = "stop"
on_failure = "ignore"
C
done
} > c.toml
unshare -Urpf --mount-proc $B --config c.toml > out.log 2>&1 & U=$!
for i in $(seq 50); do curl -sf localhost:19810/api/v1/health >/dev/null && break; sleep 0.2; done
SD=$(pgrep -P $U); echo "stormd: host pid $SD, PID 1 in its namespace"
( sleep 60; kill -9 $SD 2>/dev/null ) & GUARD=$!
for t in 0 2 4 6 8 10; do
  echo "t+$t s: zombie children of stormd: $(ps -eo stat=,ppid= | awk -v p=$SD '$1 ~ /^Z/ && $2 == p' | wc -l)"
  sleep 2
done
curl -s localhost:19810/api/v1/processes | python3 -c '
import json, sys
ps = [p for p in json.load(sys.stdin) if p["name"].startswith("one")]
print("one-shots:", len(ps), "| stopped with exit 0:", sum(p["state"] == "stopped" and p["exit_code"] == 0 for p in ps),
      "| no exit code:", sum(p["exit_code"] is None for p in ps))'
echo "'process exited with error' lines: $(grep -c 'process exited with error' out.log)"
grep 'process exited with error' out.log | head -3 | sed 's/^/  /'
kill -TERM $SD; wait $U; echo "stormd (via unshare) rc=$?"; kill $GUARD 2>/dev/null
