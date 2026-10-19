# DevUI device screen

Open **DevUI** in the OpenUI sidebar. This page manages the device's English
320×480 touchscreen separately from the WireGuard and sing-box pages. It provides
**Open DevUI screen**, **Restore stock UI**, two stock UI shortcuts (four power
presses and a top-left corner long-press), and saved preferences. Closing the
browser does not close the device screen. Leaving DevUI never disables WireGuard
or sing-box, and never changes saved device routing.

## Power button and stock UI shortcuts

Inside DevUI, a short power-button press turns the backlight off; the next short
press wakes it at the configured brightness. A short press lasts 30–799 ms.
Touch actions are ignored while dark, including touches queued across wake-up.
The display stops redrawing while asleep; agent jobs and supervisor heartbeats
continue. This controls the LCD backlight, not whole-device suspend or power-off.

Enable **Press power four times** under **Shortcuts** and click **Save DevUI
settings**. From the stock UI, make four short presses within **2.5 seconds**,
leaving no more than **0.7 seconds** between presses. The agent waits 350 ms after
the fourth release for the factory callback to settle, then uses its normal
screen startup path. A five-second cooldown prevents repeated launches.
The shortcut is disabled by default on a new installation and persists across
agent restarts after being saved.

The observer reads `pmic_pwrkey` without EVIOCGRAB and does not stop the factory
key service. Stock UI may turn its display on and off during the four presses.
Long presses, auto-repeat, incomplete sequences and dropped events cannot launch
DevUI. Events are drained while the shortcut is disabled or DevUI is already
running, so they cannot be replayed when it becomes enabled. The shortcut requires
a verified screen component and a completed factory boot handshake. It does not
modify factory menus, binaries, boot scripts, power-off or hardware-reset behavior.

Enable **Long-press the top-left corner** to open DevUI by holding one finger in
the top-left 64×64 area of the upright stock screen for **2 seconds**. The finger
may move up to 16 pixels. A second finger, leaving the area, lifting early or a
dropped event cancels the hold; one hold launches at most once, and a five-second
cooldown follows. The gesture only counts while the stock display is lit, so a
dark screen in a pocket cannot open DevUI. The observer reads the Sitronix touch
device without EVIOCGRAB, so stock UI still receives the same touches, and scales
and rotates them by 180° exactly like the screen program. Touches are drained while
the shortcut is disabled or DevUI is running, so they are never replayed. It is
disabled by default and uses the same verified startup path as the four presses.

## Online settings

| Setting | Behavior |
| --- | --- |
| Four-press shortcut | Enable or disable entry from stock UI; saved on the device |
| Corner long-press shortcut | Enable or disable the 2-second top-left hold; saved on the device |
| Brightness | 20–255, shown as a percentage; updates while awake without waking a sleeping panel |
| Idle return | 30–600 seconds; default 120 seconds, measured from touch or a short power press |
| Startup page | Overview, sing-box or WireGuard; applies on the next opening |
| Overview entries | Show or hide the sing-box and WireGuard entries on Overview |

Overview, **Stock UI** and **Back** are always available, so both entries may be
hidden. A hidden gateway cannot be the startup page. Hiding an entry only changes
the screen; it does not disable its network service. A hidden page with unsaved
work remains open until the draft or operation is resolved. Settings saved by
earlier versions are migrated when read: the old main menu becomes Overview, the
WireGuard entry stays visible if any WireGuard page was visible, and the sing-box
entry is added. The configured idle timeout discards unsaved device
routing choices and returns to stock UI, even when the display is asleep.
Recovery restores the brightness recorded before takeover; it may therefore
light the stock display again if it was previously awake.

Click **Save DevUI settings** to persist edits. The screen picks up brightness,
idle timeout and entry visibility within its two-second polling interval. Revision
checks reject stale saves from another browser session; **Reload settings**
reloads the device's saved version. Settings contain no passwords, VPN keys, HTML,
scripts or shell commands.

## Screen pages

```
Overview (home)
├─ sing-box ── status, enable/disable, current node, traffic
│   ├─ Nodes
│   ├─ Saved profiles
│   └─ Device routing
└─ WireGuard ── status, enable/disable, handshake, traffic
    ├─ Saved profiles
    └─ Device routing
```

**Back** returns to the gateway page, then to Overview. **Stock UI** is in the
Overview header.

- **Overview:** a battery icon with a proportional fill and large percentage
  (low-battery color at 20% or below, `--%` when unknown), next to the cellular
  signal: five bars, network type (`5G SA`, `5G NSA`, `4G`, `3G`), RSRP and
  operator, from the same modem status as the webpage. Without service it shows
  *No signal*. Below are battery temperature, charging state, CPU, memory and
  uptime, then one entry per visible gateway with its colored state (green running
  or connected, yellow starting or waiting, grey off, red error) and its current
  node or profile.
- **sing-box:** state, active profile, uptime, memory, per-run traffic, current
  node and enable/disable. Disabling requires confirmation. While WireGuard is on,
  **Enable** is greyed out with *Turn off WireGuard first* (and vice versa on the
  WireGuard page); the agent rejects the conflict regardless. Configuration
  imports, subscription URLs and node details stay in OpenUI.
- **Nodes:** the first selectable group of the running configuration, four per
  page, up to 32. The current node has a check mark; an automatic group such as
  URLTest shows the node it picked. Tapping a node switches immediately; **Test
  latency** measures the whole group. Each request carries a version of the node
  list, so a list that changed in the meantime is refused and redrawn instead of
  switching to the wrong node.
- **WireGuard:** configuration state, handshake age, per-run tunnel traffic and
  enable/disable. Disabling requires confirmation. A recent handshake does not
  claim verified internet access. Keys and configuration imports stay in OpenUI.
- **Saved profiles** (both gateways): up to five named profiles; sing-box rows also
  show the source (file or subscription) and node count. Switching preserves the
  current enable state and device routing; confirmation remembers the profile ID
  and gateway revision to prevent overwriting concurrent webpage edits.
- **Device routing** (both gateways): four rows per page, including saved offline
  devices. Checked devices use that gateway; others use normal networking. Edits
  apply in a batch. The new-device default policy is configured on the gateway's
  webpage. Discovery uses the same bridge, neighbor and Wi-Fi observations as
  OpenUI, not an active reachability probe. Up to 128 choices are projected.

## Build and deployment

The separate screen executable uses aarch64 musl and GPL-3.0-or-later. It talks
to the MIT-licensed agent over a root-only Unix socket. The renderer is adapted
from [33333s/u60pro-devui](https://github.com/33333s/u60pro-devui), using litehtml
0.10, FreeType 2.13.3 and GNU Unifont 17.0.05 OTF. The display layer derives from
the Atomic DRM example in
[amenekowo/mu5250_tweaking](https://github.com/amenekowo/mu5250_tweaking).
Dependency archives and the font are SHA-256 pinned. License texts are retained
under `screen/`; no proprietary device fonts or factory UI assets are bundled.

```sh
ZIG=/path/to/zig sh screen/build.sh
# Native rendering and interaction checks without hardware:
sh screen/build.sh --preview
# Add to the normal component deployment command:
# --screen build/screen/openui-screen
```

Deployment snapshots the previous components and verification marker for rollback.
Restore stock UI before replacing a running screen executable. A new screen binary
invalidates the marker until the device recovery checks pass. The webpage and
power-button shortcut both respect this gate.

The following command temporarily takes over the screen and tests backlight
sleep/wake, startup without touch, crash and stall recovery, web open/close,
duplicate open and the default 120-second idle return. It does not change
WireGuard configuration or network rules. Run it with the default 120-second
DevUI idle setting; disable the shortcut while carrying out unattended tests.
Existing login credentials are read in memory and are not printed.

```sh
python3 scripts/verify-screen.py --gateway 192.168.0.1 --port 2222 \
  --ssh-key /path/to/id_ed25519
```

After recovery verification, the following optional test checks authenticated
settings, stale-save rejection, live brightness without restarting the panel, and
a configured 30-second idle return. It restores the prior display preferences;
`--enable-shortcut` deliberately leaves the four-press shortcut enabled on success.
Do not interact with the device during the idle test.

```sh
python3 scripts/verify-devui-settings.py --gateway 192.168.0.1 --port 2222 \
  --ssh-key /path/to/id_ed25519 --enable-shortcut
```

## Interfaces and process ownership

- Authenticated `GET /api/screen`: availability, verification, lifecycle state,
  settings revision and the power-key and corner listener status.
- Authenticated `POST /api/screen` / `DELETE /api/screen`: asynchronous open and
  restore. A single startup reservation and supervisor lock prevent duplicate owners.
- Authenticated `GET /api/screen/settings` / `PUT /api/screen/settings`: typed,
  versioned preferences. Saves require `expected_revision`; conflicts return 409.
  Settings are atomically stored in `/data/local/tmp/openui-devui/settings.json`
  with mode 0600 in a 0700 directory.
- Private `/tmp/openui-screen/control.sock`: mode 0600, parent directory 0700.
  The screen receives redacted `S`/`P` (WireGuard), `B`/`Q`/`M`/`N` (sing-box
  status, profiles, node group and nodes), `D` (devices with both gateways'
  selections), `O` (system), `G` (signal), `C` and `J` records. `C` contains the
  DevUI revision, timeout, brightness, startup page and visible-entry bitmask.
  Names are stripped of control characters and shortened. No private keys, PSKs,
  endpoints, server addresses, subscription URLs or login credentials appear in
  this projection.
- WireGuard actions are narrow local `toggle`, `devices` and `profile` messages,
  and sing-box actions are `sb-toggle`, `sb-devices` and `sb-profile`; each carries
  that gateway's revision. `sb-node` and `sb-delay` carry the node-list version and
  an index instead of a node name. Jobs execute in the agent and continue after
  screen exit. Status older than ten seconds disables mutation controls.

The independent supervisor checks factory boot synchronization and records the
stock UI, touch process and brightness before taking over. The display uses RGB565
Atomic double buffers and rotates both the frame and touch coordinates by 180°.
Startup first commits a real frame, then enables the backlight and makes two
bounded follow-up commits. The ready signal follows this initialization; no touch
is needed to request the first visible frame. Normal rendering only submits
changed content. No legacy SETCRTC, `image_dump` or vendor LED ubus calls are used.

A ten-second heartbeat failure terminates the child and restores stock UI, with a
two-second termination grace period. Loss of agent contact for fifteen seconds
also ends the panel. These are process-level recovery measures, not protection
against kernel display failures, power loss or killing both supervisor and child.
Manual recovery over SSH is available:

```sh
/data/bin/openui-screen --restore
```

## Verification boundaries

Native tests render the real HTML links and font, checking navigation, saved
profiles for both gateways, node pages, the exact requests sent for each action,
WireGuard/sing-box exclusivity, concurrent revision capture, pagination, drafts,
HTML escaping, confirmation, stale controls, hidden entries, battery and signal
states and rotation. Agent tests cover settings validation, conflicts and migration,
the projection and signal mapping, four-press recognition (slow sequences, long
presses, repeats, bounce, dropped events) and the corner hold (short, moved,
outside, multi-finger, dropped, re-arming, scaling and rotation). Optimized screen
builds retain self-test assertions.

On-device `--self-test` uses synthetic events within the process; `--probe` reads
actual touch axes and checks the dedicated power input. `--power-check` performs a
supervised five-second off/on/off cycle with sysfs readback and stock recovery.
It does not inject power events into the factory input device. Recovery checks
also verify stock synchronization, HTTP access, default route and unchanged
WireGuard configuration hashes. Physical four-press and corner long-press
interaction and first-open screen appearance require confirmation on the device; a successful ioctl alone
cannot prove the LCD's visual appearance. VPN exit verification is separate.

## B31 verification record — 2026-09-14

The agent, dashboard and screen updates were deployed to the U60 Pro. Native
rendering tests, 70 agent tests, frontend build/lint/tests and API contract checks
passed. Device checks passed backlight off/on/off, startup completion without
synthetic touch, child KILL/STOP recovery, duplicate opening, webpage restoration,
120-second idle return, live brightness changes and configured 30-second return.
Saved WireGuard configuration and the default route were unchanged.

The device owner confirmed that four short power-button presses successfully
open DevUI from stock UI and the picture appears immediately, without a touch.
The four-press shortcut is enabled on this device and can be disabled from the
DevUI webpage. Brightness remains 128/255, idle return 120 seconds, startup page
Main menu, with all four menu features visible after verification.

## B31 verification record — 2026-10-19

The Overview, sing-box and corner long-press update (agent, dashboard and screen)
was deployed to the same U60 Pro. Native rendering and request assertions, 91 agent
tests, clippy, Python tests, API contract checks and frontend build/lint/tests
passed. `verify-screen.py` passed the on-device self-test, backlight off/on/off,
takeover and idle restore, child KILL/STOP recovery, web open, duplicate open and
close, and the 120-second idle return. `verify-devui-settings.py` passed
authenticated versioned settings, live brightness, private storage and the
configured 30-second return, then restored the settings. Saved WireGuard
configuration was unchanged.

The earlier settings (Main menu startup page, four menu features) migrated to
Overview with both gateway entries, keeping the four-press shortcut enabled. The
device owner confirmed that a 2-second top-left long-press and four power presses
both open DevUI from stock UI, and that Overview (battery and signal), sing-box
node switching and the WireGuard pages work. Both shortcuts are now enabled on
this device; brightness remains 128/255, idle return 120 seconds, startup page
Overview, with both entries visible.
