# DevUI integration

The menu uses the real litehtml/FreeType RGB565 renderer from
[33333s/u60pro-devui](https://github.com/33333s/u60pro-devui), pinned at
`d1d0856e19d58a2a98104e47afbdcfc51e1dc3d1`. The adapted renderer is in
`vendor/html_view.cpp`; its MIT notice is preserved. It embeds an OFL Unifont
instead of loading proprietary device fonts, and supports a macOS preview.

`ui/menu.html` and `ui/style.css` define the menu header and appearance.
`render.h` generates visible feature rows from saved OpenUI preferences, with a
battery badge and configurable idle return. `render.h` builds
the dynamic pages using escaped agent data; `actions.h` accepts only fixed
navigation, toggle, device-selection and profile-activation actions. HTML and
font assets are embedded in the screen binary for atomic updates and rollback.

The existing `panel.c` keeps its Atomic double buffers, rotated MT-B touch
handling, root-only agent socket, idle timeout and independent recovery process.
Upstream DRM SETCRTC, raw service/USB commands, power-key handling, autostart
scripts and zwrt-datad are not part of this integration. The menu provides
Overview, WireGuard, Saved profiles and Device routing; it does not install all
of the upstream DevUI system-control pages.

Build with `ZIG=/path/to/zig sh screen/build.sh`. Run `sh screen/build.sh --preview`
with a native C/C++ compiler for matching HTML rendering and interaction tests.
`build.py` downloads source archives and the font with pinned SHA-256 hashes.
The cross target is aarch64 musl; all build outputs stay under `build/`.

Device verification and recovery instructions: [docs/SCREEN.md](../../docs/SCREEN.md).
The combined screen program is GPL-3.0-or-later; dependency notices are included
in `../THIRD-PARTY-LICENSES` and deployed alongside the binary.

OpenUI provides a separate **DevUI** sidebar page for opening/restoring the screen
and editing brightness, idle return, startup page and menu visibility. Preferences
are projected through the existing root socket. The agent's non-exclusive power
observer can launch DevUI after four short presses when the optional shortcut is
enabled. The panel handles its own short-press sleep/wake. Initial display setup
commits the frame before enabling the backlight and completes two follow-up
Atomic commits before reporting readiness.
