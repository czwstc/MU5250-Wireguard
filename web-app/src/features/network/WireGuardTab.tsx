import { useCallback, useEffect, useRef, useState } from 'react'
import { api } from '../../data/api'
import type { Client, WireGuardStatus } from '../../types'
import { Button, Select, Toggle } from '../../ui/controls'
import { Card, Chip, Stat } from '../../ui/primitives'
import { toast, toastError } from '../../ui/feedback'
import ScreenControl from './ScreenControl'

function bytes(n: number) {
  return n >= 1073741824 ? `${(n / 1073741824).toFixed(2)} GB` : n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${(n / 1024).toFixed(1)} KB`
}

export default function WireGuardTab() {
  const [status, setStatus] = useState<WireGuardStatus | null>(null)
  const [clients, setClients] = useState<Client[]>([])
  const [error, setError] = useState('')
  const [mode, setMode] = useState<'all' | 'selected'>('all')
  const [macs, setMacs] = useState<string[]>([])
  const [config, setConfig] = useState('')
  const [busy, setBusy] = useState(false)
  const initialized = useRef(false)
  const editing = useRef(false)
  const editRevision = useRef(0)
  const mounted = useRef(true)
  const busyRef = useRef(false)
  const reading = useRef(false)
  const generation = useRef(0)

  const refresh = useCallback(async () => {
    if (busyRef.current || reading.current) return
    reading.current = true
    const current = generation.current
    try {
      const [s, c] = await Promise.all([api.wireguard(), api.clients()])
      if (!mounted.current || busyRef.current || current !== generation.current) return
      setStatus(s); setClients(c); setError('')
      if (!initialized.current || !editing.current) {
        setMode(s.mode); setMacs(s.macs); editRevision.current = s.revision; initialized.current = true
      }
    } catch (e) {
      if (mounted.current && current === generation.current) setError(e instanceof Error ? e.message : 'Unable to load WireGuard')
    } finally { reading.current = false }
  }, [])
  useEffect(() => {
    mounted.current = true
    void refresh()
    const timer = window.setInterval(() => { if (!document.hidden) void refresh() }, 10000)
    return () => { mounted.current = false; window.clearInterval(timer) }
  }, [refresh])

  async function save(enabled: boolean, disableOnly = false) {
    generation.current += 1
    busyRef.current = true; setBusy(true); setError('')
    try {
      const s = await api.wireguardSet(disableOnly && status ? { enabled: false, mode: status.mode, macs: status.macs, expected_revision: status.revision } : { enabled, mode, macs, expected_revision: editRevision.current, config_text: config.trim() || undefined })
      if (!mounted.current) return
      setStatus(s); setMode(s.mode); setMacs(s.macs); setConfig(''); editRevision.current = s.revision; editing.current = false
      toast(enabled ? 'WireGuard policy applied. Waiting for peer handshake.' : 'Saved. All devices use the normal network.')
    } catch (e) { if (mounted.current) { setError(e instanceof Error ? e.message : 'Unable to apply configuration'); toastError(e, 'WireGuard update failed') } }
    finally { busyRef.current = false; if (mounted.current) setBusy(false) }
  }
  async function importFile(file?: File) {
    if (!file) return
    if (file.size > 16384) { toast('Configuration must be under 16 KiB', 'err'); return }
    try { editing.current = true; setConfig(await file.text()) } catch (e) { toastError(e, 'Unable to read configuration') }
  }
  const known = new Map(clients.filter(c => c.mac).map(c => [c.mac.toLowerCase(), c]))
  for (const m of macs) if (!known.has(m)) known.set(m, { mac: m, hostname: 'Saved device (offline)' } as Client)
  const rows = [...known.values()]
  const selected = (m: string) => mode === 'all' ? !macs.includes(m) : macs.includes(m)
  const dirty = !!config.trim() || mode !== status?.mode || [...macs].sort().join() !== [...(status?.macs ?? [])].sort().join()
  const ready = !!status?.kernel_supported && !!status?.tools_installed && (!!status?.has_config || !!config.trim())
  const summary = status?.connected ? 'Recent handshake' : status?.enabled ? status.error ? 'Needs attention' : 'Waiting for handshake' : 'Off'

  return (
    <div className="space-y-4">
      <Card title="WireGuard" action={<Chip tone={status?.connected ? 'ok' : status?.enabled ? 'warn' : 'default'}>{summary}</Chip>}>
        <div className="space-y-4">
          <div className="flex items-start justify-between gap-4">
            <div>
              <p className="text-[13px] font-medium text-ink">WireGuard routing for your connected devices</p>
              <p className="mt-1 text-xs text-ink2">Route every device automatically, or choose individual devices below. A recent handshake does not verify internet access.</p>
            </div>
            <Toggle label="Enable WireGuard" checked={status?.enabled ?? false} disabled={busy || !status || (!status.enabled && !ready)} onChange={v => void save(v, !v)} />
          </div>
          {(error || status?.error) && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{error || status?.error}</p>}
          {!status && <p className="text-sm text-ink2">Loading WireGuard status…</p>}
          {status && <>
            <div className="flex flex-wrap gap-2">
              <Chip tone={status.kernel_supported ? 'ok' : 'warn'}>Kernel {status.kernel_supported ? 'ready' : 'unavailable'}</Chip>
              <Chip tone={status.tools_installed ? 'ok' : 'warn'}>Tools {status.tools_installed ? 'ready' : 'missing'}</Chip>
              {dirty && <Chip tone="warn">Unsaved changes</Chip>}
            </div>
            <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
              <Stat label="Downloaded" value={bytes(status.rx_bytes)} />
              <Stat label="Uploaded" value={bytes(status.tx_bytes)} />
              <Stat label="Handshake" value={status.latest_handshake ? `${Math.max(0, Math.round(Date.now()/1000-status.latest_handshake))}s ago` : 'Not yet'} />
              <Stat label="Tunnel MTU" value={status.mtu ?? '1280'} />
            </div>
            {status.endpoint && <p className="break-all text-xs text-ink2">{status.endpoint} · {status.address} · DNS {status.dns}</p>}
          </>}
        </div>
      </Card>

      <ScreenControl />

      <Card title={status?.has_config ? 'Replace client configuration' : 'Import client configuration'}>
        <div className="space-y-3">
          <p className="text-xs leading-relaxed text-ink2">Import a standard single-peer .conf file with an IPv4 address and AllowedIPs = 0.0.0.0/0. MTU 1280 is recommended (supported: 1280–1380). Shell hooks are not accepted. Saved private keys are never displayed again.</p>
          <input aria-label="Import WireGuard configuration file" type="file" accept=".conf,.txt" disabled={busy} className="block w-full text-xs text-ink2 file:mr-3 file:rounded-lg file:border-0 file:bg-surface2 file:px-3 file:py-2 file:text-ink" onChange={e => { void importFile(e.target.files?.[0]); e.target.value = '' }} />
          <textarea aria-label="WireGuard client configuration" spellCheck={false} autoComplete="off" disabled={busy} value={config} onChange={e => { editing.current = true; setConfig(e.target.value) }} rows={8} placeholder={status?.has_config ? 'A configuration is saved. Leave this empty to keep it, or paste a replacement.' : '[Interface]\nPrivateKey = …\nAddress = 10.0.0.2/32\nDNS = 1.1.1.1\n\n[Peer]\nPublicKey = …\nEndpoint = vpn.example.com:51820\nAllowedIPs = 0.0.0.0/0'} className="w-full resize-y rounded-lg border border-line/15 bg-surface2/50 p-3 font-mono text-xs text-ink outline-none focus:border-accent" />
          <p className="text-xs text-ink3">Only IPv4 traffic is tunneled in this version. IPv6 internet and IPv6 DNS to the router are blocked for tunnel devices to prevent direct bypass. Unselected devices keep normal IPv4 and IPv6 access.</p>
        </div>
      </Card>

      <Card title="Which devices use WireGuard?">
        <div className="space-y-3">
          <Select aria-label="Device routing mode" value={mode} disabled={busy} onChange={e => { editing.current = true; const next = e.target.value as 'all' | 'selected'; setMacs(rows.filter(c => next === 'selected' ? selected(c.mac.toLowerCase()) : !selected(c.mac.toLowerCase())).map(c => c.mac.toLowerCase())); setMode(next) }}>
            <option value="all">All devices — new devices join automatically</option>
            <option value="selected">Selected devices only — new devices use normal network</option>
          </Select>
          <p className="text-xs text-ink2">Checked: WireGuard. Unchecked: normal network. Choices follow the device MAC address, even when its IP changes. A private/random MAC is treated as a different device.</p>
          <div className="divide-y divide-line/10 rounded-lg border border-line/10">
            {rows.length === 0 ? <p className="p-4 text-sm text-ink3">No devices found. In all-devices mode, new connections will use WireGuard.</p> : rows.map(c => {
              const m = c.mac.toLowerCase(), checked = selected(m)
              return <label key={m} className="flex cursor-pointer items-center gap-3 px-3 py-3 hover:bg-surface2/50">
                <input type="checkbox" className="h-4 w-4 shrink-0 accent-accent" checked={checked} disabled={busy} onChange={() => { editing.current = true; setMacs(prev => prev.includes(m) ? prev.filter(v => v !== m) : [...prev, m]) }} />
                <span className="min-w-0 flex-1"><span className="block truncate text-[13px] font-medium text-ink">{c.hostname || c.ip || 'Connected device'}</span><span className="block break-all font-mono text-[11px] text-ink3">{c.ip || 'Offline'} · {m}</span></span>
                <Chip tone={checked ? 'accent' : 'default'}>{checked ? 'WireGuard' : 'Direct'}</Chip>
              </label>
            })}
          </div>
          <p className="rounded-lg bg-surface2/60 p-3 text-xs leading-relaxed text-ink2">When enabled, tunnel devices cannot fall back to the normal network if the VPN stops responding. DNS over IPv4 is sent through the tunnel. Router management stays accessible. Applying changes briefly pauses forwarded traffic while rules are replaced. After changing a device’s route, reconnect its apps or Wi-Fi so existing connections can restart.</p>
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" loading={busy} disabled={!status || (!status.has_config && !config.trim())} onClick={() => void save(status?.enabled ?? false)}>Save {status?.enabled ? '& apply' : 'configuration'}</Button>
            {!status?.enabled && <Button variant="outline" disabled={busy || !ready} onClick={() => void save(true)}>Save & enable</Button>}
            <Button variant="ghost" disabled={busy} onClick={() => { if (!editing.current || window.confirm('Discard unsaved changes and reload the latest settings?')) { editing.current = false; setConfig(''); void refresh() } }}>Refresh</Button>
          </div>
        </div>
      </Card>
    </div>
  )
}
