#!/usr/bin/env python3
"""Write operation: controlled display takeover and crash/timeout recovery tests.
No WireGuard configuration or network rule changes. Requires an installed agent and screen.
"""
import time
import subprocess,shlex,json,sys
import argparse, ipaddress
parser=argparse.ArgumentParser(description="Verify temporary screen takeover and recovery on your U60; enables the web entry only on success.")
parser.add_argument('--gateway',default='192.168.0.1')
parser.add_argument('--port',default=2222,type=int)
parser.add_argument('--ssh-key',required=True)
a=parser.parse_args()
ipaddress.IPv4Address(a.gateway)
ssh=['ssh','-i',a.ssh_key,'-p',str(a.port),'-o','BatchMode=yes','-o','ConnectTimeout=8','root@'+a.gateway]
def remote(cmd,data=None): return subprocess.check_output(ssh+[cmd],input=data,timeout=35)
s=remote('cat /data/local/tmp/start_zte_agent.sh').decode()
pw=next(shlex.split(l)[1].split('=',1)[1] for l in s.splitlines() if l.startswith('export ZTE_AGENT_PASSWORD='))
login=json.loads(remote(f"/usr/bin/curl -fsS --max-time 10 -H 'Content-Type: application/json' --data-binary @- http://{a.gateway}:9090/api/auth/login",json.dumps({'password':pw}).encode()))
token=login['data']['token']
def api(path,method='GET',body=None):
 opts='url = '+json.dumps(f'http://{a.gateway}:9090'+path)+'\nheader = '+json.dumps('Authorization: Bearer '+token)+'\nsilent\nshow-error\nmax-time = 20\nrequest = '+json.dumps(method)+'\n'
 if body is not None:opts+='header = "Content-Type: application/json"\ndata = '+json.dumps(json.dumps(body))+'\n'
 return json.loads(remote('/usr/bin/curl --config -',opts.encode()))

def read(cmd):return remote(cmd).decode().strip()
def health():
 assert read('pidof zte_topsw_devui')
 sync=json.loads(read('ubus call zwrt_topsw_daemon.sync get_sync_info "{}"'))
 assert sync['noSyncModuleName']=='sync success' and sync['noRegModuleName']=='register success'
 # Factory UI may legitimately dim or sleep after recovery. Verify the value
 # read back at handoff, rather than overriding its resumed brightness policy.
 assert read('cat /tmp/openui-screen/brightness-restored')==read('cat /tmp/openui-screen/brightness')
 assert read('if test -f /data/local/tmp/openui-wireguard/config.json; then sha256sum /data/local/tmp/openui-wireguard/config.json; else printf absent; fi').split()[0]==config_hash
 assert read('ip -4 route show default')==route
 assert read('/usr/bin/curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8080/')=='200'
def wait_state(expected,seconds=15):
 until=time.monotonic()+seconds
 while time.monotonic()<until:
  v=api('/api/screen')['data']
  if v['state']==expected:return v
  time.sleep(.4)
 raise AssertionError(('screen did not reach',expected,v))
def start(seconds=120):
 p=subprocess.Popen(ssh+[f'/data/bin/openui-screen --supervise {seconds} >/tmp/openui-screen/recovery-test.log 2>&1'],stdout=subprocess.DEVNULL)
 assert wait_state('active')['running'];assert read('cat /sys/class/leds/led:lcd/brightness')!='0'
 return p
config_hash=read('if test -f /data/local/tmp/openui-wireguard/config.json; then sha256sum /data/local/tmp/openui-wireguard/config.json; else printf absent; fi').split()[0]
route=read('ip -4 route show default')
try:
 print(read('/data/bin/openui-screen --self-test; /data/bin/openui-screen --ipc-check'),flush=True)
 # Exercise the same backlight function as KEY_POWER, without injecting events
 # into the factory key daemon. End asleep to check supervisor restoration.
 assert read('/data/bin/openui-screen --power-check >/tmp/openui-screen/power-test.log 2>&1; echo $?') == '0'
 power_log=read('cat /tmp/openui-screen/power-test.log')
 assert power_log.count('power key: display asleep') == 2
 assert power_log.count('power key: display awake') == 1
 assert 'display startup settled without touch' in power_log
 health(); print('PASS backlight off/on/off readback and stock restoration from sleep',flush=True)
 p=start(4);assert p.wait(timeout=15)==0;health();print('PASS short Atomic display takeover / idle restore / unchanged WG configuration',flush=True)
 for signal in ('KILL','STOP'):
  p=start()
  read('p=$(cat /tmp/openui-screen/child); test "$(readlink /proc/$p/exe)" = /data/bin/openui-screen && kill -'+signal+' "$p"')
  assert p.wait(timeout=25)!=0;health();print('PASS child '+signal+' recovery and stock sync',flush=True)
  assert api('/api/screen','DELETE')['ok'];wait_state('idle');health()
 # Enable the authenticated web entry only after actual DRM and failure recovery pass.
 read('umask 077; sha256sum /data/bin/openui-screen > /data/local/tmp/openui-screen-verified')
 assert api('/api/screen','POST')['ok'];wait_state('active');owner=read('cat /tmp/openui-screen/owner')
 assert api('/api/screen','POST')['ok'];assert owner==read('cat /tmp/openui-screen/owner')
 assert api('/api/screen','DELETE')['ok'];wait_state('idle');health();print('PASS authenticated web open, duplicate open and close',flush=True)
 assert api('/api/screen','POST')['ok'];wait_state('active');start_time=time.monotonic();print('START default 120-second idle timeout test',flush=True)
 while time.monotonic()-start_time<108:time.sleep(3)
 assert api('/api/screen')['data']['running'];wait_state('idle',25);health()
 print('PASS default 120-second timeout; stock screen, SSH, web and saved WG state preserved',flush=True)
except BaseException:
 remote('rm -f /data/local/tmp/openui-screen-verified')
 api('/api/screen','DELETE')
 raise
