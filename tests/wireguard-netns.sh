#!/bin/sh
set -eu
BASE=/tmp/openui-wg-check
R=ouwg-check-r
C=ouwg-check-c
S=ouwg-check-s
cleanup() {
 for n in "$C" "$R" "$S"; do ip netns del "$n" 2>/dev/null || true; done
 rm -f "$BASE"/key-* "$BASE"/pub-*
}
trap cleanup EXIT HUP INT TERM
for n in "$R" "$C" "$S"; do
 if ip netns list | grep -q "^$n "; then echo 'Test namespace already exists'; exit 1; fi
 ip netns add "$n"
 ip netns exec "$n" ip link set lo up
done
ip netns exec "$R" ip link add br-lan type bridge
ip netns exec "$R" ip link set br-lan up
ip netns exec "$R" ip address add 192.0.2.1/24 dev br-lan
ip netns exec "$R" ip link add cl-r type veth peer name cl-c
ip netns exec "$R" ip link set cl-c netns "$C"
ip netns exec "$R" ip link set cl-r master br-lan
ip netns exec "$R" ip link set cl-r up
ip netns exec "$C" ip link set cl-c address 02:11:22:33:44:55
ip netns exec "$C" ip address add 192.0.2.2/24 dev cl-c
ip netns exec "$C" ip link set cl-c up
ip netns exec "$C" ip route add default via 192.0.2.1
ip netns exec "$R" ip link add wan-test type veth peer name wan-peer
ip netns exec "$R" ip link set wan-peer netns "$S"
ip netns exec "$R" ip address add 198.18.0.1/30 dev wan-test
ip netns exec "$R" ip link set wan-test up
ip netns exec "$R" ip route add default via 198.18.0.2
ip netns exec "$S" ip address add 198.18.0.2/30 dev wan-peer
ip netns exec "$S" ip link set wan-peer up
ip netns exec "$S" ip address add 203.0.113.1/32 dev lo
ip netns exec "$R" sh -c 'echo 1 > /proc/sys/net/ipv4/ip_forward'
ip netns exec "$R" iptables -t nat -A POSTROUTING -o wan-test -j MASQUERADE
umask 077
"$BASE/wg" genkey > "$BASE/key-r"
"$BASE/wg" genkey > "$BASE/key-s"
"$BASE/wg" pubkey < "$BASE/key-r" > "$BASE/pub-r"
"$BASE/wg" pubkey < "$BASE/key-s" > "$BASE/pub-s"
ip netns exec "$S" ip link add wg-peer type wireguard
ip netns exec "$S" "$BASE/wg" set wg-peer private-key "$BASE/key-s" listen-port 51820 peer "$(cat "$BASE/pub-r")" allowed-ips 10.200.0.1/32
ip netns exec "$S" ip address add 10.200.0.2/24 dev wg-peer
ip netns exec "$S" ip link set wg-peer up
setup_wg() {
 ip netns exec "$R" ip link add ou-wg0 type wireguard
 ip netns exec "$R" "$BASE/wg" set ou-wg0 private-key "$BASE/key-r" peer "$(cat "$BASE/pub-s")" allowed-ips 0.0.0.0/0 endpoint 198.18.0.2:51820
 ip netns exec "$R" ip address add 10.200.0.1/24 dev ou-wg0
 ip netns exec "$R" ip link set ou-wg0 mtu 1280 up
 ip netns exec "$R" sh -c 'echo 2 > /proc/sys/net/ipv4/conf/ou-wg0/rp_filter'
}
for mode in all selected excluded; do
 setup_wg
 ip netns exec "$R" sh "$BASE/$mode.sh"
 ip netns exec "$C" ping -c 2 -W 2 203.0.113.1 >/dev/null
 count=$(ip netns exec "$R" "$BASE/wg" show ou-wg0 transfer | awk '{print $3}')
 if [ "$mode" = excluded ]; then test "$count" = 0; else test "$count" -gt 0; fi
 echo "PASS: $mode routing"
 if [ "$mode" != excluded ]; then
  ip netns exec "$R" ip link del ou-wg0
  if ip netns exec "$C" ping -c 1 -W 1 203.0.113.1 >/dev/null 2>&1; then echo 'FAIL: tunnel-down traffic escaped'; exit 1; fi
  echo 'PASS: tunnel-down blocks selected internet'
  ip netns exec "$C" ping -c 1 -W 1 192.0.2.1 >/dev/null
  echo 'PASS: local management remains reachable'
 fi
 ip netns exec "$R" sh "$BASE/cleanup.sh"
 ip netns exec "$C" ping -c 1 -W 1 203.0.113.1 >/dev/null
 echo 'PASS: disable restores direct network'
done
ip netns exec "$R" sh "$BASE/hold.sh"
if ip netns exec "$C" ping -c 1 -W 1 203.0.113.1 >/dev/null 2>&1; then echo 'FAIL: transition guard'; exit 1; fi
ip netns exec "$C" ping -c 1 -W 1 192.0.2.1 >/dev/null
ip netns exec "$R" sh "$BASE/release.sh"
ip netns exec "$C" ping -c 1 -W 1 203.0.113.1 >/dev/null
echo 'PASS: transition guard and recovery'
