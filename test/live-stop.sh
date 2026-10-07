#!/bin/bash
# Live check for #9 against a built stormd (run from the repo root after
# `cargo build`): SIGTERM then SIGKILL after stop_timeout_secs, restart waits
# for the old run, shutdown stops dependents first. Prints what it saw.
set -u
B=$PWD/target/debug/stormd; W=$(mktemp -d); cd $W
cat > c.toml <<C
[general]
log_dir = "$W/log"
[api]
bind = "127.0.0.1:19809"
[ssh]
enabled = false
[stormlog.mcast]
group = "off"
[[process]]
name = "db"
command = "/bin/sh"
args = ["-c", "trap 'if [ -e $W/app-alive ]; then echo early > $W/verdict; else echo late > $W/verdict; fi; exit 0' TERM; while :; do sleep 0.1; done"]
[[process]]
name = "app"
command = "/bin/sh"
args = ["-c", "trap 'sleep 1; rm -f $W/app-alive; echo app-flushed > $W/app; exit 3' TERM; touch $W/app-alive; while :; do sleep 0.1; done"]
depends_on = ["db"]
[[process]]
name = "stubborn"
command = "/bin/sh"
args = ["-c", "trap '' TERM; while :; do sleep 0.1; done"]
stop_timeout_secs = 2
C
timeout -k 5 60 $B --config c.toml > out.log 2>&1 & SD=$!
for i in $(seq 50); do curl -sf localhost:19809/api/v1/health >/dev/null && break; sleep 0.2; done; sleep 1
echo "== API stop of stubborn (ignores TERM, stop_timeout_secs=2)"
t=$(date +%s.%N); curl -s -XPOST localhost:19809/api/v1/processes/stubborn/stop; echo
while curl -s localhost:19809/api/v1/processes/stubborn | grep -q '"state":"stopping"'; do sleep 0.1; done
echo "stopped after $(echo "$(date +%s.%N) - $t" | bc) s: $(curl -s localhost:19809/api/v1/processes/stubborn | grep -o '"state":"[a-z]*"')"
echo "== API restart of app (TERM handler sleeps 1 s, exits 3)"
old=$(curl -s localhost:19809/api/v1/processes/app | grep -o '"pid":[0-9]*'); t=$(date +%s.%N)
curl -s -XPOST localhost:19809/api/v1/processes/app/restart; echo
echo "restart returned after $(echo "$(date +%s.%N) - $t" | bc) s; old $old new $(curl -s localhost:19809/api/v1/processes/app | grep -o '"pid":[0-9]*'); app file: $(cat app)"; rm -f app
sleep 1
echo "== SIGTERM to stormd"
t=$(date +%s.%N); kill -TERM $SD; wait $SD; rc=$?
echo "stormd rc=$rc after $(echo "$(date +%s.%N) - $t" | bc) s; verdict (db saw app alive?): $(cat verdict); app: $(cat app 2>/dev/null)"
echo "leftover sh: $(pgrep -f "$W" | wc -l)"
grep -E "stopping process|stopped by request|SIGKILL|SIGTERM" out.log | sed 's/^/  /'
