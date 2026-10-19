#!/bin/bash
# Prepare this working tree for installer testing without publishing a release.
set -euo pipefail
umask 077
ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
DESTINATION=${1:-"$ROOT_DIR/dist/install-bundle"}
[ ! -e "$DESTINATION" ] || { echo 'Choose a new output directory; an existing bundle will not be overwritten.' >&2; exit 1; }
mkdir -p "$(dirname "$DESTINATION")"
WORK=$(mktemp -d "$(dirname "$DESTINATION")/.bundle-XXXXXX")
trap 'rm -rf "$WORK"' EXIT
cd "$ROOT_DIR"
cargo zigbuild --locked --release --target aarch64-unknown-linux-musl -p zte-agent
(cd web-app && npm ci && npm run build)
python3 scripts/prepare-wireguard.py --output build/wireguard/wg
python3 scripts/prepare-singbox.py --output build/singbox/sing-box
sh screen/build.sh
python3 scripts/deploy-components.py --prepare-only --bundle-output "$WORK"
cp target/aarch64-unknown-linux-musl/release/zte-agent "$WORK/zte-agent"
tar czf "$WORK/dashboard-dist.tar.gz" -C "$ROOT_DIR/web-app/dist" . -C "$ROOT_DIR" LICENSE NOTICE licenses
cp build/wireguard/wg "$WORK/wg"
cp build/singbox/sing-box "$WORK/sing-box"
cp licenses/sing-box-LICENSE.txt "$WORK/sing-box-LICENSE"
cp build/screen/openui-screen "$WORK/openui-screen"
for name in LICENSE NOTICE THIRD-PARTY-LICENSES; do cp "screen/$name" "$WORK/screen-$name"; done
python3 - "$WORK" <<'PY'
import hashlib,json,sys,re
from pathlib import Path
root=Path(sys.argv[1])
release='v'+re.search(r'^version\s*=\s*"([^"]+)"',Path('agent/Cargo.toml').read_text(),re.M)[1]
payloads=['zte-agent','dashboard-dist.tar.gz','wg','sing-box','sing-box-LICENSE','openui-screen','screen-LICENSE','screen-NOTICE','screen-THIRD-PARTY-LICENSES']
checksums={name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in payloads}
(root/'sha256sums.txt').write_text(''.join(f'{checksum}  {name}\n' for name,checksum in checksums.items()))
files={name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in [*payloads,'sha256sums.txt','dropbear.ipk','uhttpd.ipk']}
(root/'bundle.json').write_text(json.dumps({'format_version':2,'release':release,'target':{'model':'MU5250','hardware':'MU5250_HW1.0','firmware':'CN_ZTE_MU5250V1.0.0B31'},'files':files},indent=2))
PY
mv "$WORK" "$DESTINATION"
echo "Offline bundle ready: $DESTINATION"
