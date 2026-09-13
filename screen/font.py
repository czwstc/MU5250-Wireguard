#!/usr/bin/env python3
"""Generate a BMP bitmap header from checksum-pinned GNU Unifont 17.0.05.
The generated OpenUI Bitmap font uses the SIL Open Font License (see FONT-LICENSE).
"""
import gzip, hashlib, pathlib, sys, urllib.request
out = pathlib.Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
p = out / 'unifont.hex.gz'
if not p.exists():
    p.write_bytes(urllib.request.urlopen('https://unifoundry.com/pub/unifont/unifont-17.0.05/font-builds/unifont-17.0.05.hex.gz', timeout=60).read())
if hashlib.sha256(p.read_bytes()).hexdigest() != '2ae5311c8e123e9e85f5331cd012aa99757071df23243f1487fdbf8f3acd86be':
    raise ValueError('Font checksum mismatch')
with (out / 'font.h').open('w') as f:
    f.write('/* OpenUI Bitmap: generated from GNU Unifont 17.0.05; SIL OFL 1.1. */\nstatic const unsigned char font[65536][33] = {\n')
    for line in gzip.decompress(p.read_bytes()).decode().splitlines():
        code, data = line.split(':'); code = int(code, 16); raw = bytes.fromhex(data)
        if code >= 65536 or len(raw) not in (16, 32): continue
        width = len(raw) // 2
        f.write(f'[{code}]={{' + ','.join(map(str, bytes([width])+raw)) + '},\n')
    f.write('};\n')
