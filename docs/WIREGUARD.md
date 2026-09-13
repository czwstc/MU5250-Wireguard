# WireGuard gateway

Network → WireGuard manages one kernel WireGuard client tunnel. Import a standard
single-peer `.conf`, save it, choose devices and enable. A new installation is off.
The router itself keeps its original WAN route; forwarded devices use the policy.

## Device selection

- **All devices:** all current and future `br-lan` clients use the tunnel. Unchecking
  a device records a MAC exclusion, so that device uses the normal network.
- **Selected devices:** checked MAC addresses use the tunnel; all other clients
  use the normal network. Offline selections remain visible and editable.
- Selection follows MAC addresses, not DHCP leases. Private/random MAC changes
  create a new identity. Devices behind another NAT router cannot be distinguished.

## Configuration and limits

Requires `/sys/module/wireguard`, iproute2 and `/data/bin/wg`. The supplied preparer
extracts only `wg` from a checksum-pinned official OpenWrt package; it does not run
opkg, replace kernel modules, or install netifd/init.d hooks.

The importer accepts one `[Interface]` and one `[Peer]`, with PrivateKey, Address,
DNS, MTU, PublicKey, optional PresharedKey, Endpoint, AllowedIPs and
PersistentKeepalive. An IPv4 address and `AllowedIPs = 0.0.0.0/0` are required.
Endpoint is an IPv4 address or DNS hostname followed by port. Default keepalive is
25 seconds, default DNS 1.1.1.1, default MTU 1280 (allowed 1280–1380 for this cellular
WAN). Arbitrary hooks and additional peers are rejected.

This version forwards **IPv4 only**. IPv6 entries in Address/AllowedIPs are not
applied: IPv6 internet and IPv6 DNS queries to the router are blocked for tunnel
clients to prevent bypass. Direct clients keep their stock IPv6 behavior.
IPv4 TCP/UDP DNS (port 53) from tunnel clients is DNATed to the configured resolver
through WireGuard; the stock router resolver is not globally changed.

Once policy is installed, an unreachable fallback route prevents selected traffic
falling back to WAN if the tunnel disappears. A nonresponding peer stalls tunnel
traffic. Turning WireGuard off deliberately restores normal internet.
Changing policy briefly pauses all forwarded LAN traffic and client DNS while
rebuilding rules; router management and router-originated traffic remain available.
Reconnect client applications/Wi-Fi after changes because established connections
may retain old conntrack/NAT state or vendor acceleration state.

The agent restores enabled settings when it starts and repairs missing policy
hooks every 30 seconds. This is not an always-on boot firewall: before the agent
starts, or while the stock firewall is reloading, the stock network may forward
traffic. Do not interpret it as a leak-proof policy during boot or firmware resets.
Vendor hardware acceleration and a real remote VPN endpoint need end-to-end
validation on the user's network; an isolated namespace test does not prove them.

## Ownership and recovery

Only `/data/local/tmp/openui-wireguard/` stores feature state (directory 0700,
files 0600). GET responses redact private and preshared keys. The existing dashboard
still uses LAN HTTP; importing a key inherits that transport limitation.

Reserved resources: interface `ou-wg0`, routing table/rule priority `10420`, mark
`0x40000000/0x40000000`, firewall chains prefixed `OUWG_`. Initial activation refuses
pre-existing resources. Main routes and stock chains are not flushed or rewritten.
The feature is supervised by the existing agent boot path; no new boot hook is added.

Apply errors attempt to restore the previous configuration. A transition guard
remains in place if recovery fails. Use the dashboard's Off switch to remove owned
rules and restore direct traffic. If the agent is unavailable, connect by SSH,
restore the previous agent snapshot, and reboot if temporary rules need clearing.
Preserve the config directory if you want to retain the client configuration.

## Build and deployment

```sh
python3 scripts/prepare-wireguard.py --output build/wireguard/wg
# Build the agent and dashboard using the existing repository workflow, then:
python3 scripts/deploy-components.py --agent target/aarch64-unknown-linux-musl/release/zte-agent \
  --dashboard build/dashboard-dist.tar.gz --wireguard-tools build/wireguard/wg \
  --ssh-key '/path/to/existing/key' --dry-run
# Repeat without --dry-run to deploy through the existing snapshot/recovery flow.
```

Use the existing agent credentials when deploying; deployment must not silently
reset them. Future upstream installers may overwrite this local feature.

## Validation

Rust tests cover import restrictions, invalid identifiers, MAC selection and route
policy generation. `OPENUI_WG_TEST_POLICIES=/absolute/test/directory cargo test -p
zte-agent wireguard` exports the generated policies for a network namespace test.
`tests/wireguard-netns.sh` uses those scripts and the pinned `wg` in
`/tmp/openui-wg-check` on a Linux host with WireGuard, veth and network namespaces.
It generates disposable keys, checks all/selected/excluded forwarding, tunnel-down
blocking, local management reachability, disabling and transition guard recovery,
then removes its namespaces and disposable keys. It does not touch main-namespace
routes or firewall rules.
