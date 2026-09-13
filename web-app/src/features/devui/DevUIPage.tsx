import { useEffect, useRef, useState } from 'react'
import { api } from '../../data/api'
import type { DevUISettings, DevUIPageId, ScreenStatus } from '../../types'
import { Button, Select, Toggle } from '../../ui/controls'
import { Card, Chip } from '../../ui/primitives'
import { confirm } from '../../ui/feedback'

const pages: { id: DevUIPageId; title: string; description: string }[] = [
  { id: 'overview', title: 'Overview', description: 'Battery, temperature, uptime and system load' },
  { id: 'wireguard', title: 'WireGuard', description: 'Tunnel status, traffic and on / off control' },
  { id: 'profiles', title: 'Saved profiles', description: 'Switch between your saved VPN profiles' },
  { id: 'devices', title: 'Device routing', description: 'Choose which devices use WireGuard' },
]
const labels: Record<string, string> = { idle: 'Stock UI', starting: 'Opening', active: 'DevUI active', restoring: 'Restoring', error: 'Recovery needs attention' }
export default function DevUIPage() {
  const [status, setStatus] = useState<ScreenStatus | null>(null)
  const [settings, setSettings] = useState<DevUISettings | null>(null)
  const [draft, setDraft] = useState<DevUISettings | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [loadError, setLoadError] = useState('')
  const [message, setMessage] = useState('')
  const mutating = useRef(false)
  const editing = useRef(false)
  const mounted = useRef(false)
  const generation = useRef(0)
  useEffect(() => {
    mounted.current = true
    let reading = false
    const refresh = async () => {
      if (reading || mutating.current || document.hidden) return
      reading = true
      const current = generation.current
      try {
        const [s, c] = await Promise.all([api.screen(), api.screenSettings()])
        if (mounted.current && current === generation.current) {
          setStatus(s); setSettings(c); setLoadError('')
          if (!editing.current) setDraft(c)
        }
      } catch (e) { if (mounted.current) setLoadError(e instanceof Error ? e.message : 'Unable to read DevUI settings') }
      finally { reading = false }
    }
    void refresh()
    const timer = window.setInterval(() => void refresh(), 2000)
    return () => { mounted.current = false; window.clearInterval(timer) }
  }, [])
  function edit(patch: Partial<DevUISettings>) {
    editing.current = true; setMessage(''); setDraft(d => d ? { ...d, ...patch } : d)
  }
  async function save() {
    if (!draft) return
    mutating.current = true; setBusy(true); setError(''); setMessage(''); generation.current++
    try {
      const { revision, ...values } = draft
      const saved = await api.screenSettingsSet({ ...values, expected_revision: revision })
      if (mounted.current) { editing.current = false; setDraft(saved); setSettings(saved); setMessage('Saved. Display and menu changes apply within two seconds. The startup page applies next time DevUI opens.') }
    } catch (e) { if (mounted.current) setError(e instanceof Error ? e.message : 'Unable to save DevUI settings') }
    finally { generation.current++; mutating.current = false; if (mounted.current) setBusy(false) }
  }
  async function reload() {
    if (editing.current && !(await confirm({ title: 'Discard unsaved DevUI settings?', body: 'Reload the settings saved on the device.', confirmLabel: 'Reload' }))) return
    mutating.current = true; setBusy(true); generation.current++; setError('')
    try { const c = await api.screenSettings(); if (mounted.current) { editing.current = false; setDraft(c); setSettings(c); setMessage('Settings reloaded.') } }
    catch (e) { if (mounted.current) setError(e instanceof Error ? e.message : 'Unable to reload settings') }
    finally { generation.current++; mutating.current = false; if (mounted.current) setBusy(false) }
  }
  async function change(open: boolean) {
    mutating.current = true; setBusy(true); setError(''); generation.current++
    try {
      await (open ? api.screenOpen() : api.screenClose())
      if (mounted.current) setStatus(s => s ? { ...s, running: true, state: open ? 'starting' : 'restoring' } : s)
    } catch (e) { if (mounted.current) setError(e instanceof Error ? e.message : 'Screen action failed') }
    finally { generation.current++; mutating.current = false; if (mounted.current) setBusy(false) }
  }
  const stale = draft && settings && draft.revision !== settings.revision
  return <div className="space-y-4">
    <div><h1 className="text-xl font-bold text-ink">DevUI</h1><p className="mt-0.5 text-[13px] text-ink2">Device screen, power-button shortcut and live menu settings</p></div>
    {(error || loadError) && <p role="alert" className="rounded-lg border border-danger/20 bg-danger/5 p-3 text-sm text-danger">{error || loadError}</p>}
    {message && <p role="status" className="rounded-lg bg-accent/5 p-3 text-sm text-ink">{message}</p>}
    <Card title="Device screen" action={<Chip tone={status?.state === 'error' ? 'warn' : status?.running ? 'accent' : 'default'}>{status ? labels[status.state] ?? status.state : 'Loading'}</Chip>}>
      <div className="space-y-3">
        <p className="text-[13px] text-ink2">Open the English touchscreen menu on your U60 Pro. Short-press the power button to sleep or wake the display. Returning to the stock UI keeps your WireGuard settings and network connection.</p>
        {status && (!status.available || !status.verified) && <p className="text-xs text-ink2">The screen component must be installed and pass device recovery verification before it can open.</p>}
        <div className="flex flex-wrap gap-2">
          <Button variant="primary" disabled={busy || !status?.available || !status.verified || status.running} onClick={() => void change(true)}>Open DevUI screen</Button>
          <Button variant="outline" disabled={busy || !status?.available || (!status.running && status.state !== 'error')} onClick={() => void change(false)}>Restore stock UI</Button>
        </div>
      </div>
    </Card>
    <Card title="Power-button shortcut" action={<Chip tone={settings?.shortcut_enabled ? 'accent' : 'default'}>{settings?.shortcut_enabled ? 'Enabled' : 'Disabled'}</Chip>}>
      <div className="space-y-3">
        <div className="flex items-start justify-between gap-4"><div><p className="text-[13px] font-medium text-ink">Press four times to open DevUI</p><p className="mt-1 text-xs leading-relaxed text-ink2">From the stock UI, make four short presses within 2.5 seconds, with no more than 0.7 seconds between presses. Long presses keep their factory behavior.</p></div><Toggle label="Enable four-press shortcut" checked={draft?.shortcut_enabled ?? false} disabled={busy || !draft} onChange={v => edit({ shortcut_enabled: v })} /></div>
        <p className="text-xs text-ink3">{status?.shortcut?.available ? status.shortcut.message : 'Power-key listener is not available on this device.'} The stock display may turn on and off as you press.</p>
      </div>
    </Card>
    <Card title="Display preferences">
      {draft ? <div className="grid gap-5 sm:grid-cols-2">
        <label className="block text-[13px] text-ink">Brightness <span className="float-right font-mono">{Math.round(draft.brightness / 255 * 100)}%</span><input aria-label="DevUI brightness" className="mt-3 w-full accent-accent" type="range" min="20" max="255" value={draft.brightness} disabled={busy} onChange={e => edit({ brightness: Number(e.target.value) })} /><span className="mt-1 block text-xs text-ink3">Applied live; changing brightness does not wake a sleeping screen.</span></label>
        <label className="block text-[13px] text-ink">Return to stock UI after<Select aria-label="DevUI idle timeout" className="mt-2 w-full" value={draft.idle_seconds} disabled={busy} onChange={e => edit({ idle_seconds: Number(e.target.value) })}>{Array.from(new Set([30, 60, 120, 300, 600, draft.idle_seconds])).sort((a, b) => a - b).map(n => <option key={n} value={n}>{n < 60 ? `${n} seconds` : `${n / 60} ${n === 60 ? 'minute' : 'minutes'}`}</option>)}</Select><span className="mt-2 block text-xs text-ink3">Unsaved device routing choices are discarded on idle return.</span></label>
        <label className="block text-[13px] text-ink">Startup page<Select aria-label="DevUI startup page" className="mt-2 w-full" value={draft.home_page} disabled={busy} onChange={e => edit({ home_page: e.target.value as DevUISettings['home_page'] })}><option value="menu">Main menu</option>{pages.filter(p => draft.menu_items.includes(p.id)).map(p => <option key={p.id} value={p.id}>{p.title}</option>)}</Select></label>
      </div> : <p className="text-sm text-ink2">Loading display preferences…</p>}
    </Card>
    <Card title="Menu features">
      <p className="mb-3 text-xs text-ink2">Choose the controls shown on the device. Hiding a menu item does not disable its network feature. Stock UI and Back remain available.</p>
      <div className="divide-y divide-line/10">{pages.map(p => <label key={p.id} className="flex cursor-pointer items-center gap-3 py-3"><input type="checkbox" className="h-4 w-4 accent-accent" checked={draft?.menu_items.includes(p.id) ?? false} disabled={busy || !draft} onChange={e => { if (!draft) return; const items = e.target.checked ? [...draft.menu_items, p.id] : draft.menu_items.filter(id => id !== p.id); edit({ menu_items: items, home_page: draft.home_page !== 'menu' && !items.includes(draft.home_page) ? 'menu' : draft.home_page }) }} /><span><span className="block text-[13px] font-medium text-ink">{p.title}</span><span className="text-xs text-ink2">{p.description}</span></span></label>)}</div>
    </Card>
    <div className="sticky bottom-2 rounded-xl border border-line/10 bg-surface p-3 shadow-sm">
      {stale && <p role="alert" className="mb-2 text-xs text-danger">Settings changed in another session. Reload before saving.</p>}
      {draft?.menu_items.length === 0 && <p role="alert" className="mb-2 text-xs text-danger">Keep at least one menu feature enabled.</p>}
      <div className="flex flex-wrap items-center gap-2"><Button variant="primary" disabled={busy || !editing.current || !draft?.menu_items.length || !!stale} onClick={() => void save()}>{busy ? 'Working…' : 'Save DevUI settings'}</Button><Button variant="outline" disabled={busy} onClick={() => void reload()}>Reload settings</Button><span className="text-xs text-ink3">{editing.current ? 'Unsaved changes' : 'Settings saved on device'}</span></div>
    </div>
  </div>
}
