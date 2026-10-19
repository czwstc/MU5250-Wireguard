# sing-box gateway

The **sing-box** sidebar page (between WireGuard and Modem) runs one
[sing-box](https://sing-box.sagernet.org/) proxy for chosen LAN devices. Import a
sing-box JSON configuration or subscription, choose devices, enable it, then pick
nodes. A new installation is off. The router's own traffic keeps its normal WAN route.

sing-box and WireGuard are **mutually exclusive**: only one of them can route
devices at a time. Turn one off before enabling the other.

## Saved configurations (up to 5)

Under **Add configuration**, choose a source and give the profile a unique name
(1–48 characters):

- **JSON file or text** — a complete sing-box configuration for the installed
  version (1.14). A filename becomes the initial name.
- **Subscription URL** — an `https://` or `http://` link that returns sing-box JSON.
  The router downloads it with its own connection (`/usr/bin/curl`, 30 s, 512 KiB
  maximum, user agent `sing-box/<version>`). Share links (`vless://`, `ss://` …),
  base64 link lists and Clash/Mihomo YAML are **not** supported.

Every import is validated and checked with `sing-box check` before it is saved.
**Saved configurations** shows the current profile, its source, node count and
last update. Choose an entry, then **Switch & apply** (while enabled) or **Use
configuration** (while off). **Update subscription** downloads the link again;
updating the current profile while enabled restarts sing-box with the new
configuration. Renaming, saving a backup or deleting an inactive profile does not
touch the running proxy. The active profile cannot be deleted while sing-box runs.

Subscription links and imported configurations can contain credentials. They are
stored root-only on the router and never returned: the page shows only the
subscription host and the node count.

### What the router changes in an imported configuration

| Part | Handling |
| --- | --- |
| `inbounds` | Replaced by two: a `redirect` inbound on port `9096` for TCP and a `tun` inbound (`ou-sb0`, `172.19.0.1/30`, no `auto_route`) for UDP and other traffic. Imported TUN and redirect/TPROXY inbound tags are kept so rules that reference them still match. |
| `route.rules` | Kept, after three injected rules: DNS hijack for port 53 on both inbounds, `sniff`, and DNS hijack for sniffed DNS. `route.default_mark` is removed. |
| `outbounds`, `endpoints`, `dns`, rule sets | Kept. Without any `selector`, an `OpenUI` selector over all nodes is added and becomes `route.final` when none is set. |
| `log`, `experimental` | Replaced: warnings to the page log; Clash API on `127.0.0.1:9095` with a per-agent random secret; `cache_file` in the state directory, separated per profile. |
| `services`, `ntp.write_to_system` | Removed. |
| Rejected | Any `*_path` or `*_directory` field, `routing_mark`, local rule sets, `tor` outbounds, `system` endpoints, duplicate tags, configurations without proxy nodes. |

Use remote or inline rule sets. Configurations written for older sing-box versions
(legacy `dns`/`block` outbounds, inbound sniff fields) are rejected by `sing-box check`;
the error is shown on the page.

### API

Authenticated `PUT /api/singbox` always carries the latest `expected_revision`
(stale revisions return 409). A profile action cannot be combined with routing changes:

```json
{"expected_revision":3,"profile":{"action":"save","name":"Home","config_text":"{…}"}}
{"expected_revision":4,"profile":{"action":"subscribe","name":"Airport","url":"https://…"}}
{"expected_revision":5,"profile":{"action":"refresh","id":2}}
{"expected_revision":6,"profile":{"action":"activate","id":2}}
{"expected_revision":7,"profile":{"action":"rename","id":2,"name":"Travel"}}
{"expected_revision":8,"profile":{"action":"delete","id":1}}
{"expected_revision":9,"enabled":true,"mode":"selected","macs":["02:11:22:33:44:55"]}
```

`GET /api/singbox` returns status: `version`, `binary_installed`, `kernel_supported`, `ready`, `starting`,
`running`, `pid`, `uptime`, `memory_rss`, traffic totals, `error`, profile metadata
and `wireguard_enabled`. Node selection uses `GET /api/singbox/proxies`,
`PUT /api/singbox/proxies` (`{"group":"…","name":"…"}`) and
`POST /api/singbox/delay` (`{"group":"…"}`); recent log lines come from
`GET /api/singbox/logs`.

## Node selection

While sing-box runs, **Node selection** lists the configuration's groups in order.
Click a node in a selector group to switch immediately; there is no restart and no
firewall change. Existing connections may keep the previous node until they reconnect.
URLTest groups choose automatically and are shown read-only. **Test latency**
measures every node of a group (up to 128, eight at a time, 5 s each) against
`https://www.gstatic.com/generate_204`; *Timeout* means the test failed. Choices
are kept per profile in sing-box's cache file and survive restarts.

## Device selection

Identical to WireGuard: **All devices** (unchecked devices are recorded as
exclusions) or **Selected devices only**, by MAC address. Offline selections stay
visible. Choices are shared by all sing-box profiles and are separate from the
WireGuard selections.

## Routing behavior and limits

Requires `/data/bin/sing-box`, kernel TUN (`/dev/net/tun`), the iptables `REDIRECT`
target, iproute2 and iptables/ip6tables. Proxied devices' packets are marked in
`mangle PREROUTING`, then:

- **TCP** is redirected (`nat REDIRECT`) to sing-box's `redirect` inbound on port 9096.
  The kernel terminates the connection and hands it to sing-box as a normal socket, so
  most traffic avoids a userspace network stack. Only redirected connections may reach
  the port; direct connections to it are refused.
- **UDP, ICMP and everything else** is routed by the mark (table `10430`) into
  sing-box's TUN `ou-sb0`.
- **Why not TPROXY:** the stock firewall needs `net.bridge.bridge-nf-call-iptables=1`
  for its Wi-Fi isolation rules. With it, iptables runs in bridge context and the
  kernel drops TPROXY's socket assignment, so TPROXY never reaches sing-box. NAT
  redirection and mark-based routing are unaffected. The stock UU game accelerator
  uses the same TCP-redirect plus UDP-TUN split.

sing-box connects out through the router's normal WAN route.

- **IPv4 only.** IPv6 internet and IPv6 DNS to the router are blocked for proxied
  devices to prevent bypass. Direct devices keep stock IPv6 behavior.
- **DNS.** IPv4 UDP port 53 from proxied devices (to the router or any other server) is
  DNATed to `172.19.0.2` inside the TUN; TCP port 53 goes through the redirect. sing-box
  answers both using the configuration's `dns` section, so the stock dnsmasq is not used.
- **Startup.** On a first start sing-box downloads the configuration's remote rule
  sets before it opens its inbounds; with 16 rule sets this took about 8–10 seconds
  on the test modem. The page shows *Starting* meanwhile. The agent adds the TUN route
  as soon as the interface appears. Only a crash within the first 3 seconds fails an
  apply (and restores the previous configuration); a start that is not ready after
  5 minutes is restarted.
- **Fail-closed.** The policy is installed before sing-box starts. Until it is ready,
  redirected TCP is refused and routed traffic hits an unreachable fallback route;
  marked traffic that would otherwise be forwarded to WAN is rejected. If sing-box exits, proxied devices are
  blocked (not sent directly) while the agent restarts it; the supervisor checks every
  10 seconds. Turning sing-box off deliberately restores normal internet.
- Changing policy briefly pauses all forwarded LAN traffic and client DNS while rules
  are rebuilt. Router management and router-originated traffic remain available.
- Like WireGuard, this is not an always-on boot firewall: before the agent starts or
  while the stock firewall reloads, traffic may be forwarded directly.
- sing-box runs with a 96 MiB soft memory limit (`GOMEMLIMIT`). Large rule sets or
  many connections still raise memory use; watch the **Memory** value.
- Naive outbounds need `libcronet`, which is not installed; use other protocols.

## Ownership and recovery

Feature state lives only in `/data/local/tmp/openui-singbox/` (directory 0700,
files 0600): `config.json` (settings and profile metadata), `profiles/<sha256>.json`
(imported configurations, stored by content so a failed switch can restore the
previous file), `run.json` (generated runtime configuration), `cache.db` and the
process ID file. The binary is `/data/bin/sing-box`; its license notice is in
`/data/local/tmp/openui-singbox-licenses/`.

Reserved resources: TUN interface `ou-sb0` with network `172.19.0.0/30`, routing
table/rule priority `10430`, mark `0x20000000/0x20000000`, firewall chains prefixed
`OUSB_`, port `9096` (redirect listener; direct connections refused) and loopback port
`9095` (Clash API). Do not let the stock UU accelerator handle a device that sing-box
proxies; both mark and redirect the same traffic. Initial activation refuses pre-existing
resources. The agent supervises sing-box; no boot hook is added. A sing-box process
left behind by a previous agent is stopped when the agent starts.

Apply errors restore the previous profile and policy. A transition guard remains
if recovery fails; use the page's Off switch to remove owned rules and restore direct
traffic. If the agent is unavailable, connect by SSH, run `killall sing-box`, restore
the previous agent snapshot, and reboot if temporary rules need clearing.

## Build and deployment

```sh
python3 scripts/prepare-singbox.py --output build/singbox/sing-box
python3 scripts/deploy-components.py --singbox-binary build/singbox/sing-box \
  --ssh-key '/path/to/existing/key' --dry-run
# Repeat without --dry-run to deploy through the existing snapshot/recovery flow.
```

The preparer downloads the checksum-pinned official release
`sing-box-1.14.3-linux-arm64-musl.tar.gz` (statically linked, so it does not depend
on the firmware C library) and extracts only the binary (about 82 MiB). Deployment
checks free space for the binary and its rollback copy. A running proxy keeps the
old binary until sing-box is next started (turn it off and on, or restart the agent).
The complete offline bundle includes `sing-box` and `sing-box-LICENSE`.

sing-box is GPL-3.0-or-later and is run unmodified as a separate program; see
[licenses/device-components.txt](../licenses/device-components.txt). This project
is not affiliated with sing-box.

## Validation

Rust tests cover configuration rewriting and rejection rules, profile limits and
revisions, subscription refresh, secret redaction, the redirect/TUN policy and
node-name escaping. The shared policy code is checked to generate byte-identical WireGuard rules.

`OPENUI_SB_TEST_CONFIG=/absolute/dir cargo test -p zte-agent singbox` exports generated
runtime configurations (validate them with `sing-box check -c <file>` from the same
release); `OPENUI_SB_TEST_POLICIES=/absolute/dir` exports the policy scripts.
`tests/singbox-netns.sh <dir>` takes a directory with both exports (`e2e.json`
plus the scripts) and the sing-box binary. It builds client, router and WAN network
namespaces, with `br-lan` as a **real bridge and bridge netfilter on** (as on the
modem; `BRNF=0` turns it off), and checks: policy application and health, blocking
while the proxy is down, TCP through REDIRECT → sing-box → a SOCKS hop, UDP DNS to
the router and to a remote server answered via the TUN, non-DNS UDP reaching
sing-box through the TUN, direct connections to the redirect port refused,
fail-closed after sing-box stops, direct access for excluded devices, and complete
cleanup. It never touches the host namespace and verifies that its firewall and
routing state are unchanged. A plain veth named `br-lan` is not enough: the earlier
TPROXY design passed that way and failed behind the real bridge.

On 2026-10-18 this script passed on the B31 test modem (kernel 5.15.194) with the
official sing-box 1.14.3 musl build. A live check with one Wi-Fi client and the
user's own profile also passed (TCP via REDIRECT, DNS answered by sing-box, UDP
into the TUN, complete cleanup). Interaction with vendor hardware acceleration
under sustained load still needs observation.
