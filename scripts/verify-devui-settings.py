#!/usr/bin/env python3
"""Device write test: DevUI settings, live brightness, idle return and shortcut opt-in.
Temporarily changes DevUI preferences, then restores them. Never changes WireGuard.
With --enable-shortcut, leave the four-press shortcut enabled after successful tests.
Run verify-screen.py first. Do not touch the display during the 30-second idle test.
"""
import argparse
import ipaddress
import json
import shlex
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--gateway', default='192.168.0.1')
parser.add_argument('--port', type=int, default=2222)
parser.add_argument('--ssh-key', required=True)
parser.add_argument('--enable-shortcut', action='store_true')
a = parser.parse_args()
ipaddress.IPv4Address(a.gateway)
ssh = ['ssh', '-i', a.ssh_key, '-p', str(a.port), '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8', 'root@' + a.gateway]

def remote(command, data=None):
    return subprocess.check_output(ssh + [command], input=data, timeout=35).decode().strip()

startup = remote('cat /data/local/tmp/start_zte_agent.sh')
password = next(shlex.split(l)[1].split('=', 1)[1] for l in startup.splitlines() if l.startswith('export ZTE_AGENT_PASSWORD='))
login = remote(f'/usr/bin/curl -fsS --max-time 10 -H "Content-Type: application/json" --data-binary @- http://{a.gateway}:9090/api/auth/login', json.dumps({'password': password}).encode())
token = json.loads(login)['data']['token']

def api(path, method='GET', body=None):
    opts = 'url = ' + json.dumps(f'http://{a.gateway}:9090' + path) + '\nheader = ' + json.dumps('Authorization: Bearer ' + token) + '\nsilent\nshow-error\nmax-time = 20\nrequest = ' + json.dumps(method) + '\n'
    if body is not None:
        opts += 'header = "Content-Type: application/json"\ndata = ' + json.dumps(json.dumps(body)) + '\n'
    return json.loads(remote('/usr/bin/curl --config -', opts.encode()))

def settings():
    result = api('/api/screen/settings')
    assert result['ok']
    return result['data']

def save(values):
    body = dict(values)
    body['expected_revision'] = body.pop('revision')
    result = api('/api/screen/settings', 'PUT', body)
    assert result['ok'], result
    return result['data']

def wait(predicate, timeout=12):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        if predicate():
            return
        time.sleep(.4)
    raise AssertionError('Device condition timed out')

screen = api('/api/screen')['data']
assert screen['verified'] and not screen['running'], 'Restore stock UI and verify the screen first'
original = settings()
config_hash = remote('sha256sum /data/local/tmp/openui-wireguard/config.json').split()[0]
route = remote('ip -4 route show default')
succeeded = False
try:
    for method in ('GET', 'PUT'):
        code = remote('/usr/bin/curl -s -o /dev/null -w "%{http_code}" -X ' + method + f' http://{a.gateway}:9090/api/screen/settings')
        assert code == '401', code
    invalid = dict(original, expected_revision=original['revision'], brightness=0)
    invalid.pop('revision')
    assert not api('/api/screen/settings', 'PUT', invalid)['ok']
    assert settings() == original
    trial = save(dict(original, shortcut_enabled=False, brightness=160, idle_seconds=30, home_page='overview', menu_items=['overview', 'wireguard']))
    stale = dict(invalid, brightness=200)
    assert not api('/api/screen/settings', 'PUT', stale)['ok']
    assert settings() == trial
    assert remote('stat -c "%a" /data/local/tmp/openui-devui/settings.json') == '600'
    assert remote('stat -c "%a" /data/local/tmp/openui-devui') == '700'
    assert api('/api/screen', 'POST')['ok']
    wait(lambda: api('/api/screen')['data']['state'] == 'active')
    wait(lambda: remote('cat /sys/class/leds/led:lcd/brightness') == '160')
    owner = remote('cat /tmp/openui-screen/owner')
    trial = save(dict(trial, brightness=192, home_page='menu', menu_items=original['menu_items']))
    wait(lambda: remote('cat /sys/class/leds/led:lcd/brightness') == '192')
    assert remote('cat /tmp/openui-screen/owner') == owner
    print('PASS authenticated, versioned settings; live brightness without panel restart; private storage', flush=True)
    wait(lambda: not api('/api/screen')['data']['running'], timeout=40)
    assert remote('pidof zte_topsw_devui')
    print('PASS configured 30-second idle return and stock UI restoration', flush=True)
    assert remote('sha256sum /data/local/tmp/openui-wireguard/config.json').split()[0] == config_hash
    assert remote('ip -4 route show default') == route
    succeeded = True
finally:
    api('/api/screen', 'DELETE')
    wait(lambda: not api('/api/screen')['data']['running'])
    restored = save(dict(original, revision=settings()['revision'], shortcut_enabled=True if succeeded and a.enable_shortcut else original['shortcut_enabled']))
if succeeded:
    wait(lambda: api('/api/screen')['data']['shortcut']['available'])
    assert settings() == restored
    print('PASS settings restored, WG configuration unchanged; four-press shortcut ' + ('enabled' if restored['shortcut_enabled'] else 'disabled'), flush=True)
    print('Physical four-press interaction and immediate LCD appearance still require an on-device check.', flush=True)
