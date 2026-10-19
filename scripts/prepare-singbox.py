#!/usr/bin/env python3
"""Extract only sing-box from a pinned official release archive; no opkg/init.d changes.

The static musl build runs regardless of the firmware C library."""
import argparse
import hashlib
import io
from pathlib import Path
import tarfile
import urllib.request

VERSION = '1.14.3'
NAME = f'sing-box-{VERSION}-linux-arm64-musl'
URL = f'https://github.com/SagerNet/sing-box/releases/download/v{VERSION}/{NAME}.tar.gz'
SHA256 = '12d6d67a4edf0dd54590865b1f4f79cd09ae043a02d648e9d1b5fccc8446ee07'
LIMIT = 64 * 1024 * 1024


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', default='build/singbox/sing-box')
    args = parser.parse_args()
    with urllib.request.urlopen(URL, timeout=60) as response:
        package = response.read(LIMIT + 1)
    if len(package) > LIMIT or hashlib.sha256(package).hexdigest() != SHA256:
        raise SystemExit('sing-box package checksum mismatch')
    with tarfile.open(fileobj=io.BytesIO(package), mode='r:gz') as archive:
        entry = archive.getmember(f'{NAME}/sing-box')
        if not entry.isfile() or entry.size > 128 * 1024 * 1024:
            raise SystemExit('Unexpected sing-box archive entry')
        binary = archive.extractfile(entry).read()
    if binary[:6] != b'\x7fELF\x02\x01' or int.from_bytes(binary[18:20], 'little') != 183:
        raise SystemExit('sing-box is not an aarch64 ELF64 executable')
    path = Path(args.output)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(binary)
    path.chmod(0o700)
    print(f'Checksum verified; extracted sing-box {VERSION} to {path}')


if __name__ == '__main__':
    main()
