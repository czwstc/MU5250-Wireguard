# research/ — quarantined exploit & injection tooling

**DO NOT RUN THESE AGAINST A DEVICE YOU CARE ABOUT.**

These scripts were written during the July 2026 attempt to regain ADB access
on locked firmware. They deliberately probe or abuse firmware weaknesses and
are the class of tooling that bricked the original device. They are kept only
as a record of what was tried; nothing in this directory is used by
`setup.sh`, `deploy.sh`, `deploy-dashboard.sh`, or the agent.

| Script | What it does | Why it is dangerous |
|---|---|---|
| `zacs.py` | Rogue TR-069 ACS server; queues CWMP RPCs incl. `SetParameterValues` | SPV can rewrite firmware-managed config remotely; the device may also apply ACS-pushed parameters on its own schedule |
| `zrce.py` | Command-injection confirmation via `dnsquery_server`; writes a marker into firewall UCI config as root | Executes arbitrary commands as root outside rpcd ACL |
| `zinj.py` | Command injection via the ping diag target field, read back from `/log/PingMessages` | Arbitrary command execution as root |
| `zdns.py` | Injection probing of `dnsquery_server` | Same sink as zrce |
| `zstrings.py` | Charset-probes DDNS / LAN hostname setters for injection; writes to those configs | Mutates live router config while probing |
| `zgap.py` | Probes `fac_close`/`fac_reset`/`fac_reboot`, remote-ACL toggle, FOTA `start_update` | `fac_reset`/`fac_reboot` are destructive (gated behind `DANGER=1`); `start_update` can force a firmware flash |
| `zadb.py` | Tries `adb_switch`/`fac_open`/hidden-page login with candidate credentials | Factory-method abuse; unpredictable on locked firmware |
| `zhidden.py` | Hidden-page login with factory sticker password, then `zwrt_bsp.usb set {mode:debug}` | Bypasses the removed ADB toggle path |
| `zwrite.py` | Probes the write gate on `zwrt_router.api` setters (writes DDNS config) | Mutates live router config |

## Sanctioned path instead

For ADB/root on locked firmware use the reviewed, gated path in `scripts/`:

1. `scripts/zunlock.py` — config backup/restore unlock (`--dry-run` first)
2. `setup.sh` — agent install
3. `scripts/zharden.sh` — post-unlock hardening

Design rule (2026-07-21): **shell/ssh/adb only.** No boot hooks outside
`/etc/rc.local`, no firewall includes/hooks, no init.d changes to daemons
listed in `zte_topsw_daemon.conf`. See `docs/SAFETY.md` at the repo root.
