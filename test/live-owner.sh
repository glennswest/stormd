#!/bin/bash
# Live check for #30 against a built stormd (run from the repo root after
# `cargo build`): `[ssh] enabled` + `owner`. On this host (not a stormcos node)
# the CloudID refresh must start; with /proc/cmdline bind-mounted to a stormcos
# command line (unprivileged mount namespace, `unshare -Urm`) it must be
# refused with one ERROR while stormd still starts its processes and serves
# its API. Prints what it saw; exit 1 on a mismatch.
set -u
B=${CARGO_TARGET_DIR:-$PWD/target}/debug/stormd; W=$(mktemp -d); cd $W; rc=0
cat > c.toml <<C
[general]
log_dir = "$W/log"
[api]
bind = "127.0.0.1:19830"
[ssh]
enabled = true
bind = "127.0.0.1:19831"
host_key = "$W/hostkey"
owner = "ns"
cloudid_url = "http://127.0.0.1:1"
[stormlog.mcast]
group = "off"
[[process]]
name = "sleeper"
command = "/bin/sleep"
args = ["600"]
C
echo "console=tty0 root=/dev/ublkb0 rd.stormblock.slab=/dev/sda rd.stormblock.assimilate=any" > node-cmdline
run() { # $1 label, rest: command prefix
  local l=$1; shift
  "$@" $B --config c.toml > $l.log 2>&1 & local p=$!
  local up=no; for i in $(seq 50); do curl -sf localhost:19830/api/v1/health >/dev/null && { up=yes; break; }; sleep 0.2; done
  local st=$(curl -s localhost:19830/api/v1/processes | python3 -c 'import json,sys; print([p["state"] for p in json.load(sys.stdin)])' 2>/dev/null)
  local ssh=no; (exec 3<>/dev/tcp/127.0.0.1/19831) 2>/dev/null && ssh=yes
  echo "$l: api up=$up, processes=$st, ssh port open=$ssh"
  echo "  refused lines: $(grep -c 'owner is refused on a stormcos node' $l.log), refresh started: $(grep -c 'starting CloudID SSH key refresh' $l.log)"
  grep -E 'owner is refused|starting CloudID' $l.log | head -2 | sed 's/^/  | /'
  pkill -TERM -f "$B --config c.toml"; wait $p 2>/dev/null
  [ $up = yes ] || rc=1
}
echo "this host's cmdline has stormblock words: $(tr ' ' '\n' </proc/cmdline | grep -c '^\(rd\.\)\?stormblock\.')"
run host
grep -q 'starting CloudID SSH key refresh' host.log && ! grep -q 'owner is refused' host.log || { echo "FAIL: host"; rc=1; }
run node unshare -Urm sh -c 'mount --bind "$0" /proc/cmdline && echo "  (ns cmdline: $(cat /proc/cmdline))" >&2 && exec "$@"' $W/node-cmdline
grep -q 'owner is refused on a stormcos node' node.log && ! grep -q 'starting CloudID SSH key refresh' node.log || { echo "FAIL: node"; rc=1; }
grep -h 'ns cmdline' node.log
echo "live-owner rc=$rc"; exit $rc
