#!/bin/sh
# Isolated sing-box gateway check (TCP via REDIRECT, UDP via TUN). Client, router
# and "WAN" live in throwaway network namespaces; br-lan is a real bridge with
# bridge netfilter on, as on the modem. The host namespace is only read to prove
# it is unchanged. BASE must contain the sing-box binary, e2e.json from
# OPENUI_SB_TEST_CONFIG and the scripts from OPENUI_SB_TEST_POLICIES.
set -u
BASE=${1:-/tmp/openui-sb-check}
B=$BASE/sing-box
R="ip netns exec ousb-check-r"; C="ip netns exec ousb-check-c"; W="ip netns exec ousb-check-w"
fail=0
pass() { echo "PASS $1"; }
bad() { echo "FAIL $1: $2"; fail=1; }
snapshot() { (iptables -t mangle -S; iptables -S; iptables -t nat -S; ip6tables -S; ip rule; ip route show table all; ip -o link) 2>&1 | md5sum | cut -d' ' -f1; }
had_netns_dir=0; [ -d /var/run/netns ] && had_netns_dir=1
cleanup() {
 for n in ousb-check-r ousb-check-c ousb-check-w; do
  for pid in $(ip netns pids "$n" 2>/dev/null); do kill -9 "$pid" 2>/dev/null; done
  ip netns del "$n" 2>/dev/null
 done
 # `ip netns add` mounts an empty tmpfs on /var/run/netns; restore the original state.
 if [ "$had_netns_dir" = 0 ] && [ -d /var/run/netns ] && [ -z "$(ls -A /var/run/netns)" ]; then
  umount /var/run/netns 2>/dev/null; rmdir /var/run/netns 2>/dev/null
 fi
 rm -rf "$BASE/r" "$BASE/w" "$BASE/server.json" "$BASE/router.json" "$BASE"/*.log
 [ "$before" = "$(snapshot)" ] && echo "PASS host namespace unchanged" || echo "FAIL host namespace changed"
}
before=$(snapshot)
trap cleanup EXIT HUP INT TERM
for n in ousb-check-r ousb-check-c ousb-check-w; do
 if ip netns list | grep -q "^$n"; then echo 'Test namespace already exists'; exit 1; fi
 ip netns add "$n" || exit 1
done
$R ip link add br-lan type bridge && $R ip link add p0 type veth peer name c0 && $R ip link set p0 master br-lan && $R ip link set p0 up && $R ip link set c0 netns ousb-check-c || exit 1
$R sh -c "echo ${BRNF:-1} > /proc/sys/net/bridge/bridge-nf-call-iptables" || exit 1
echo "bridge-nf-call-iptables in router namespace: $($R cat /proc/sys/net/bridge/bridge-nf-call-iptables)"
$R ip link add wan0 type veth peer name w0 && $R ip link set w0 netns ousb-check-w || exit 1
for X in "$R" "$C" "$W"; do $X ip link set lo up; done
$C ip link set c0 address 02:11:22:33:44:55
$R ip address add 192.168.77.1/24 dev br-lan; $R ip link set br-lan up
$R ip address add 10.99.0.1/24 dev wan0; $R ip link set wan0 up
$R sh -c 'echo 1 > /proc/sys/net/ipv4/ip_forward'
$C ip address add 192.168.77.2/24 dev c0; $C ip link set c0 up; $C ip route add default via 192.168.77.1
$W ip address add 10.99.0.2/24 dev w0; $W ip link set w0 up; $W ip route add 192.168.77.0/24 via 10.99.0.1
mkdir -p "$BASE/r" "$BASE/w"
# The WAN side is a second sing-box: a SOCKS hop plus an HTTP endpoint (Clash API).
printf '%s\n' '{"log":{"level":"warn"},"inbounds":[{"type":"mixed","listen":"10.99.0.2","listen_port":1080}],"outbounds":[{"type":"direct"}],"experimental":{"clash_api":{"external_controller":"10.99.0.2:9090"}}}' > "$BASE/server.json"
sed 's/"level":"warn"/"level":"info"/' "$BASE/e2e.json" > "$BASE/router.json"
$W "$B" run -c "$BASE/server.json" -D "$BASE/w" > "$BASE/w.log" 2>&1 &
sleep 3
fetch() { $C curl -s -o /dev/null -w '%{http_code}' --max-time "${2:-5}" "${1:-http://10.99.0.2:9090/}"; echo " $?"; }
answer() { $C nslookup test.openui "$1" 2>&1 | grep -A2 '^Name:' | grep -q '10.99.0.2'; }
refused() { case "${1#* }" in 0) return 1;; *) return 0;; esac; }

r=$(fetch); [ "$r" = "200 0" ] && pass "direct path before policy" || bad "direct path before policy" "$r"
$R sh "$BASE/hold.sh" && $R sh "$BASE/selected.sh" && $R sh "$BASE/release.sh" && pass "selected policy applies" || { bad "selected policy applies" "script error"; exit 1; }
r=$(fetch "" 4); refused "$r" && pass "TCP blocked while proxy is down ($r)" || bad "TCP blocked while proxy is down" "$r"
answer 192.168.77.1 && bad "DNS blocked while proxy is down" "answered" || pass "DNS blocked while proxy is down"

$R "$B" run -c "$BASE/router.json" -D "$BASE/r" > "$BASE/r.log" 2>&1 &
i=0; until { $R test -e /sys/class/net/ou-sb0 && $R grep -q ' 00000000:2388 00000000:0000 0A' /proc/net/tcp; } || [ $i -ge 100 ]; do sleep 0.2; i=$((i+1)); done
[ $i -lt 100 ] && pass "sing-box ready: TUN ou-sb0 and redirect listener" || bad "sing-box ready" "not ready"
# What the agent does once the TUN exists.
$R sh -c 'echo 2 > /proc/sys/net/ipv4/conf/ou-sb0/rp_filter' && $R sh "$BASE/route.sh" && pass "TUN route attached" || bad "TUN route attached" "script error"
$R sh "$BASE/health.sh" && pass "health check with proxy running" || bad "health check with proxy running" "nonzero"
body=$($C curl -s --max-time 8 http://10.99.0.2:9090/)
case "$body" in *hello*) pass "TCP via REDIRECT -> sing-box -> SOCKS hop";; *) bad "TCP via REDIRECT" "body='$body'";; esac
n=$($R iptables -t nat -L OUSB_DNS -v -n -x | awk '/REDIRECT/ {print $1}')
[ "${n:-0}" -gt 0 ] && pass "TCP matched REDIRECT rule ($n packets)" || bad "TCP REDIRECT counter" "${n:-0}"
answer 192.168.77.1 && pass "UDP DNS to router answered by sing-box via TUN" || bad "UDP DNS to router" "no answer"
answer 10.99.0.2 && pass "UDP DNS to remote server answered by sing-box via TUN" || bad "UDP DNS to remote server" "no answer"
$C sh -c 'tftp -g -r openui-probe -l /dev/null 10.99.0.2 >/dev/null 2>&1 &' ; sleep 3
grep -q '10.99.0.2:69' "$BASE/r.log" && pass "non-DNS UDP reached sing-box through the TUN" || bad "non-DNS UDP via TUN" "no UDP connection to 10.99.0.2:69 in sing-box log"
r=$(fetch http://192.168.77.1:9096/ 4); refused "$r" && pass "direct connection to the redirect port refused ($r)" || bad "direct connection to the redirect port refused" "$r"
$C ping -c 1 -W 2 10.99.0.2 >/dev/null 2>&1 && echo "INFO ping through sing-box TUN: reply" || echo "INFO ping through sing-box TUN: no reply"

for pid in $(ip netns pids ousb-check-r); do kill "$pid"; done
sleep 2
r=$(fetch "" 4); refused "$r" && pass "TCP fail-closed after sing-box stops ($r)" || bad "TCP fail-closed after sing-box stops" "$r"
answer 10.99.0.2 && bad "UDP fail-closed after sing-box stops" "answered" || pass "UDP fail-closed after sing-box stops"

$R sh "$BASE/hold.sh" && $R sh "$BASE/cleanup.sh" && $R sh "$BASE/all.sh" && $R sh "$BASE/release.sh" && pass "all-devices policy with exclusion applies" || bad "all-devices policy" "script error"
r=$(fetch); [ "$r" = "200 0" ] && pass "excluded device goes direct" || bad "excluded device goes direct" "$r"
$R sh "$BASE/cleanup.sh" && pass "cleanup runs" || bad "cleanup runs" "script error"
left=$( ($R iptables -t mangle -S; $R iptables -S; $R iptables -t nat -S; $R ip6tables -S; $R ip rule; $R ip route show table 10430) 2>/dev/null | grep -c -E 'OUSB|10430|0x20000000')
[ "$left" = "0" ] && pass "cleanup leaves no rules" || bad "cleanup leaves no rules" "$left left"
r=$(fetch); [ "$r" = "200 0" ] && pass "direct path after cleanup" || bad "direct path after cleanup" "$r"

if [ $fail -ne 0 ]; then echo "--- router sing-box log"; tail -n 25 "$BASE/r.log"; fi
echo "RESULT fail=$fail"
[ $fail -eq 0 ]
