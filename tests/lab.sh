#!/usr/bin/env bash
# Two-NAT check on a pvelab node: alice (NAT A) and bob (NAT B) reach each other only via the relay.
# Bob joins, both chat, bob shares a fake model, alice's endpoint streams from it.
# Usage: tests/lab.sh   (needs `pvelab up`; creates guests relay, nata, natb, alice, bob on first run)
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q
BIN=/usr/local/bin/potluck

guest() { # NAME IP [NET1] [NET0]: create once, then rewire; restart after a node reboot
  if pvelab ls | grep -qw "$1"; then
    pvelab ls | awk -v n="$1" '$3 == n && $2 == "stopped" {print $1}' | xargs -r pvelab pct start
    return
  fi
  pvelab new "$1" --ip "$2" >/dev/null
  id=$(pvelab ls | awk -v n="$1" '$3 == n {print $1}')
  [ -n "${3:-}" ] && pvelab pct set "$id" --net1 "$3"
  [ -n "${4:-}" ] && pvelab pct set "$id" --net0 "$4"
  return 0
}
pvelab ssh 'for b in vmbr2 vmbr3; do ip link show $b >/dev/null 2>&1 || { ip link add $b type bridge; ip link set $b up; }; done'
guest relay 10.10.0.10
guest nata 10.10.0.11 name=eth1,bridge=vmbr2,ip=192.168.2.1/24
guest natb 10.10.0.12 name=eth1,bridge=vmbr3,ip=192.168.3.1/24
guest alice 10.10.0.13 "" name=eth0,bridge=vmbr2,ip=192.168.2.10/24,gw=192.168.2.1
guest bob 10.10.0.14 "" name=eth0,bridge=vmbr3,ip=192.168.3.10/24,gw=192.168.3.1
for r in nata natb; do # masquerade eth1 -> eth0
  pvelab exec $r -- sh -c 'command -v nft >/dev/null || apt-get install -y -qq nftables >/dev/null
    sysctl -qw net.ipv4.ip_forward=1; nft list table ip nat >/dev/null 2>&1 ||
    { nft add table ip nat && nft add chain ip nat post "{ type nat hook postrouting priority 100; }" &&
      nft add rule ip nat post oifname eth0 masquerade; }'
done
pvelab exec alice -- sh -c 'command -v curl >/dev/null || apt-get install -y -qq curl >/dev/null'

for n in relay alice bob; do
  pvelab exec $n -- pkill -x potluck || true
  pvelab exec $n -- pkill -x tail || true
  pvelab exec $n -- rm -rf /root/.config/potluck /root/in /root/out.log
  pvelab push $n target/release/potluck $BIN
  pvelab exec $n -- chmod +x $BIN
done
pvelab push bob tests/lab-backend.py /root/backend.py
pvelab exec bob -- sh -c 'pgrep -fx "python3 /root/backend.py" >/dev/null || nohup python3 /root/backend.py >/dev/null 2>&1 &'

pvelab exec relay -- sh -c "nohup $BIN relay > /root/out.log 2>&1 &"
wait_for() { # NAME PATTERN
  timeout 60 sh -c "until pvelab exec $1 -- grep -qF -- '$2' /root/out.log; do sleep 1; done" ||
    { echo "FAIL $1 never printed: $2"; pvelab exec "$1" -- grep -v listening /root/out.log; exit 1; }
  echo "ok $1: $2"
}
wait_for relay 10.10.0.10/tcp
R=$(pvelab exec relay -- grep 10.10.0.10/tcp /root/out.log | cut -d' ' -f2)
start() { # NAME ARGS...
  local n=$1; shift
  pvelab exec "$n" -- sh -c "touch /root/in; nohup sh -c 'tail -f /root/in | $BIN $* --relay $R' > /root/out.log 2>&1 &"
}
say() { pvelab exec "$1" -- sh -c "echo '$2' >> /root/in"; }

pvelab exec alice -- $BIN create friends --nick alice >/dev/null
start alice chat friends
wait_for alice p2p-circuit
say alice /invite
wait_for alice 'ticket saved'
start bob join "$(pvelab exec alice -- cat /root/.config/potluck/invite.txt)" --nick bob
wait_for alice 'bob joined'
say bob 'hi from NAT B'
wait_for alice '<bob> hi from NAT B'
say alice 'hi back'
wait_for bob '<alice> hi back'
say bob '/share m http://127.0.0.1:8000'
say alice '/token ide'
wait_for alice 'token ide '
TOK=$(pvelab exec alice -- grep 'token ide ' /root/out.log | cut -d' ' -f4)
api() { pvelab exec alice -- curl -s -H "authorization: Bearer $TOK" -H 'content-type: application/json' "$@"; }
timeout 60 sh -c "until pvelab exec alice -- curl -s -H 'authorization: Bearer $TOK' http://127.0.0.1:7777/r/friends/v1/models | grep -q '\"m\"'; do sleep 1; done"
echo "ok alice: sees model m"
out=$(api -d '{"model":"m","stream":true,"messages":[]}' http://127.0.0.1:7777/r/friends/v1/chat/completions)
grep -q '"lo"' <<< "$out" && grep -q '\[DONE\]' <<< "$out" || { echo "FAIL completion: $out"; exit 1; }
echo "ok alice: streamed completion from bob"
pvelab exec alice -- ss -Htn state established | grep -vq 10.10.0.10 && echo "ok alice: hole-punched a direct path (DCUtR)"
echo PASS
