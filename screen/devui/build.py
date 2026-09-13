#!/usr/bin/env python3
"""Build the DevUI renderer + OpenUI host; pinned sources, isolated native/cross objects."""
import concurrent.futures, hashlib, json, os, pathlib, shlex, shutil, subprocess, sys, tarfile, urllib.request
ROOT = pathlib.Path(__file__).resolve().parents[2]
os.chdir(ROOT)
DEPS = ROOT / 'build/devui-deps'
DEPS.mkdir(parents=True, exist_ok=True)
PINS = {
 'litehtml': ('https://codeload.github.com/litehtml/litehtml/tar.gz/9bc84b8b8d15a4e50f18b327aa30955048b441c2', 'ff09151e75079bad74cb4f1dc5411e4cbc39452a9e86834df7c56289833dc53f'),
 'freetype': ('https://codeload.github.com/freetype/freetype/tar.gz/refs/tags/VER-2-13-3', 'bc5c898e4756d373e0d991bab053036c5eb2aa7c0d5c67e8662ddc6da40c4103'),
 'unifont': ('https://unifoundry.com/pub/unifont/unifont-17.0.05/font-builds/unifont-17.0.05.otf', '85701ab9b1e251ee16f4df00b13f22eac311d72b7dab427a7d975fe7f5064702'),
}
def source(name):
 url, digest = PINS[name]
 p = DEPS / (name + ('.otf' if name == 'unifont' else '.tar.gz'))
 if not p.exists(): p.write_bytes(urllib.request.urlopen(url, timeout=60).read())
 if hashlib.sha256(p.read_bytes()).hexdigest() != digest: raise RuntimeError(name + ' checksum mismatch')
 if name == 'unifont': return p
 with tarfile.open(p) as t:
  top = t.getnames()[0].split('/')[0]
  if not (DEPS / top).exists():
   for m in t.getmembers():
    if not (DEPS / m.name).resolve().is_relative_to(DEPS.resolve()) or m.issym() or m.islnk():
     raise RuntimeError('Unsafe archive member')
   t.extractall(DEPS)
 return DEPS / top
lh, ft, font = (source(n) for n in PINS)
preview = '--preview' in sys.argv
out = ROOT / ('build/devui-native' if preview else 'build/devui-aarch64')
out.mkdir(parents=True, exist_ok=True)
cc = shlex.split(os.environ.get('CC', 'cc')) if preview else [os.environ.get('ZIG','zig'), 'cc', '-target', 'aarch64-linux-musl']
cxx = shlex.split(os.environ.get('CXX','c++')) if preview else [os.environ.get('ZIG','zig'), 'c++', '-target', 'aarch64-linux-musl']
modules = ['autofit_module_class','tt_driver_class','cff_driver_class','psaux_module_class','psnames_module_class','pshinter_module_class','sfnt_module_class','ft_smooth_renderer_class']
(out/'ftmodule_min.h').write_text('\n'.join('FT_USE_MODULE( %s, %s )' % ('FT_Driver_ClassRec' if m in ('tt_driver_class','cff_driver_class') else 'FT_Renderer_Class' if m=='ft_smooth_renderer_class' else 'FT_Module_Class',m) for m in modules))
# Embed an OFL font so device and preview use identical glyphs, no vendor assets.
font_header = out / 'devui_font.h'
if not font_header.exists():
 raw=font.read_bytes();font_header.write_text('static const unsigned char devui_font[] = {\n'+','.join(map(str,raw))+'\n};\n')
# HTML/CSS is compiled into the component so deployment/rollback stays one-file atomic.
ui_header=out/'devui_assets.h'
ui_header.write_text('\n'.join('static const char '+p.stem.replace('-','_')+'_asset[] = '+json.dumps(p.read_text())+';' for p in sorted((ROOT/'screen/devui/ui').glob('*'))))
incs=['-I'+str(out),'-I'+str(ft/'include'),'-I'+str(lh/'include'),'-I'+str(lh/'include/litehtml'),'-I'+str(lh/'src/gumbo/include'),'-I'+str(lh/'src/gumbo/include/gumbo'),'-Iscreen/devui/vendor']
common=['-O2','-ffunction-sections','-fdata-sections']+incs
jobs=[]
for p in sorted((lh/'src').glob('*.cpp')):jobs.append((p,cxx,['-std=c++17'], 'lh_'))
for p in sorted((lh/'src/gumbo').glob('*.c')):jobs.append((p,cc,[], 'gumbo_'))
ftsrc='base/ftbase base/ftsystem base/ftinit base/ftdebug base/ftbbox base/ftbitmap base/ftglyph base/ftmm cache/ftcache autofit/autofit truetype/truetype cff/cff psaux/psaux psnames/psnames pshinter/pshinter sfnt/sfnt smooth/smooth gzip/ftgzip'.split()
for s in ftsrc:jobs.append((ft/'src'/(s+'.c'),cc,['-DFT2_BUILD_LIBRARY','-DFT_CONFIG_MODULES_H="ftmodule_min.h"'], 'ft_'))
jobs.append((ROOT/'screen/devui/vendor/html_view.cpp',cxx,['-std=c++17'],'app_'))
# Warnings as errors on our host; upstream rendering/dependencies keep their own warnings.
# Zig release optimization defines NDEBUG: keep on-device --self-test assertions active.
jobs.append((ROOT/'screen/panel.c',cc,['-UNDEBUG','-std=c11','-Wall','-Wextra','-Werror','-Wno-unused-function']+(['-DSCREEN_PREVIEW'] if preview else []),'app_'))
def compile_one(job):
 p, compiler, flags, prefix=job
 obj=out/(prefix+p.stem+'.o'); stamp=obj.with_suffix('.sha256')
 command=compiler+common+flags+['-c',str(p),'-o',str(obj)]
 dep=b''
 if prefix=='app_':
  dep=b''.join(x.read_bytes() for x in sorted((ROOT/'screen').rglob('*.h')))+ui_header.read_bytes()
 key=hashlib.sha256(p.read_bytes()+json.dumps(command).encode()+dep).hexdigest()
 if not obj.exists() or not stamp.exists() or stamp.read_text()!=key:
  r=subprocess.run(command,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  if r.returncode: raise RuntimeError(str(p)+'\n'+r.stdout.decode(errors='replace')[:8000])
  stamp.write_text(key)
 return str(obj)
print('Building DevUI '+('native preview' if preview else 'aarch64')+' ('+str(len(jobs))+' units)',flush=True)
with concurrent.futures.ThreadPoolExecutor(max_workers=int(os.environ.get('BUILD_JOBS','4'))) as pool:
 objs=list(pool.map(compile_one,jobs))
output=ROOT/('build/screen/preview' if preview else 'build/screen/openui-screen')
output.parent.mkdir(parents=True,exist_ok=True)
subprocess.run(cxx+objs+(['-Wl,-dead_strip'] if preview and sys.platform=='darwin' else ['-Wl,--gc-sections'])+([] if preview else ['-static','-s'])+['-pthread','-lm','-o',str(output)],check=True)
print('Built '+str(output),flush=True)
if preview:subprocess.run([str(output),str(output.parent)],check=True)
