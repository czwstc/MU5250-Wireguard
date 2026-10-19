import type { Client } from '../types'
import { Select } from './controls'
import { Chip } from './primitives'

export type RoutingMode = 'all' | 'selected'

/** MAC-based device picker shared by the gateway pages. In "all" mode `macs`
 *  holds exclusions; in "selected" mode it holds the routed devices. */
export default function DeviceRouting({
  feature,
  mode,
  macs,
  clients,
  disabled,
  onChange,
}: {
  feature: string
  mode: RoutingMode
  macs: string[]
  clients: Client[]
  disabled: boolean
  onChange: (mode: RoutingMode, macs: string[]) => void
}) {
  const known = new Map(clients.filter(c => c.mac).map(c => [c.mac.toLowerCase(), c]))
  for (const m of macs) if (!known.has(m)) known.set(m, { mac: m, hostname: 'Saved device (offline)' } as Client)
  const rows = [...known.values()]
  const selected = (m: string) => mode === 'all' ? !macs.includes(m) : macs.includes(m)
  return <>
    <Select aria-label="Device routing mode" value={mode} disabled={disabled} onChange={e => { const next = e.target.value as RoutingMode; onChange(next, rows.filter(c => next === 'selected' ? selected(c.mac.toLowerCase()) : !selected(c.mac.toLowerCase())).map(c => c.mac.toLowerCase())) }}>
      <option value="all">All devices — new devices join automatically</option>
      <option value="selected">Selected devices only — new devices use normal network</option>
    </Select>
    <p className="text-xs text-ink2">Checked: {feature}. Unchecked: normal network. Choices follow the device MAC address, even when its IP changes. A private/random MAC is treated as a different device.</p>
    <div className="divide-y divide-line/10 rounded-lg border border-line/10">
      {rows.length === 0 ? <p className="p-4 text-sm text-ink3">No devices found. In all-devices mode, new connections will use {feature}.</p> : rows.map(c => {
        const m = c.mac.toLowerCase(), checked = selected(m)
        return <label key={m} className="flex cursor-pointer items-center gap-3 px-3 py-3 hover:bg-surface2/50">
          <input type="checkbox" className="h-4 w-4 shrink-0 accent-accent" checked={checked} disabled={disabled} onChange={() => onChange(mode, macs.includes(m) ? macs.filter(v => v !== m) : [...macs, m])} />
          <span className="min-w-0 flex-1"><span className="block truncate text-[13px] font-medium text-ink">{c.hostname || c.ip || 'Connected device'}</span><span className="block break-all font-mono text-[11px] text-ink3">{c.ip || 'Offline'} · {m}</span></span>
          <Chip tone={checked ? 'accent' : 'default'}>{checked ? feature : 'Direct'}</Chip>
        </label>
      })}
    </div>
  </>
}
