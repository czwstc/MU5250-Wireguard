#!/usr/bin/env python3
"""Extract only wg from a pinned upstream OpenWrt package; no opkg/init.d changes."""
import argparse
import hashlib
import io
from pathlib import Path
import tarfile
import urllib.request

URL = 'https://downloads.openwrt.org/releases/23.05.5/packages/aarch64_generic/base/wireguard-tools_1.0.20210914-2_aarch64_generic.ipk'
SHA256 = 'd632063235bfccdf635bdcbee180ee44dc26d1bf221ea6f41095d9b77b40913b'

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', default='build/wireguard/wg')
    args = parser.parse_args()
    with urllib.request.urlopen(URL, timeout=30) as response:
        package = response.read(1024 * 1024)
    if hashlib.sha256(package).hexdigest() != SHA256:
        raise SystemExit('WireGuard package checksum mismatch')
    with tarfile.open(fileobj=io.BytesIO(package), mode='r:gz') as archive:
        data = archive.extractfile('./data.tar.gz').read()
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
        binary = archive.extractfile('./usr/bin/wg').read()
    path = Path(args.output)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(binary)
    path.chmod(0o700)
    print(f'Checksum verified; extracted wg to {path}')

if __name__ == '__main__':
    main()
