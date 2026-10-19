import { useCallback, useEffect, useRef, useState } from 'react'
import { api } from '../../data/api'
import type { Client, SingBoxGroup, SingBoxProfileAction, SingBoxStatus } from '../../types'
import { Button, Input, Segmented, Select, Toggle } from '../../ui/controls'
import DeviceRouting from '../../ui/DeviceRouting'
import { Card, Chip, Stat } from '../../ui/primitives'
import { confirm, toast, toastError } from '../../ui/feedback'

function bytes(n: number) {
  return n >= 1073741824 ? `${(n / 1073741824).toFixed(2)} GB` : n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${(n / 1024).toFixed(1)} KB`
}
function duration(s: number) {
  const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600), m = Math.floor(s % 3600 / 60)
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`
}
function ago(t: number) {
  return t ? `${duration(Math.max(0, Math.round(Date.now() / 1000 - t)))} ago` : 'never'
}
function latency(delay: number | null) {
  if (delay === null) return { text: '—', className: 'text-ink3' }
  if (delay === 0) return { text: 'Timeout', className: 'text-danger' }
  return { text: `${delay} ms`, className: delay < 300 ? 'text-ok' : delay < 800 ? 'text-warn' : 'text-danger' }
}

export default function SingBoxTab() {
  const [status, setStatus] = useState<SingBoxStatus | null>(null)
  const [clients, setClients] = useState<Client[]>([])
  const [groups, setGroups] = useState<SingBoxGroup[]>([])
  const [error, setError] = useState('')
  const [mode, setMode] = useState<'all' | 'selected'>('all')
  const [macs, setMacs] = useState<string[]>([])
  const [source, setSource] = useState<'file' | 'subscription'>('file')
  const [config, setConfig] = useState('')
  const [url, setUrl] = useState('')
  const [profileName, setProfileName] = useState('')
  const [profileId, setProfileId] = useState('')
  const [renameDraft, setRenameDraft] = useState<{id: number; name: string; revision: number} | null>(null)
  const [busy, setBusy] = useState(false)
  const [nodeBusy, setNodeBusy] = useState('')
  const [logs, setLogs] = useState<string[] | null>(null)
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
      const [s, c, p] = await Promise.all([api.singbox(), api.clients(), api.singboxProxies().catch(() => null)])
      if (!mounted.current || busyRef.current || current !== generation.current) return
      setStatus(s); setClients(c); setError('')
      if (p) setGroups(p.groups)
      if (!initialized.current || !editing.current) {
        setMode(s.mode); setMacs(s.macs); editRevision.current = s.revision; initialized.current = true
      }
    } catch (e) {
      if (mounted.current && current === generation.current) setError(e instanceof Error ? e.message : 'Unable to load sing-box')
    } finally { reading.current = false }
  }, [])
  useEffect(() => {
    mounted.current = true
    void refresh()
    const timer = window.setInterval(() => { if (!document.hidden) void refresh() }, 10000)
    return () => { mounted.current = false; window.clearInterval(timer) }
  }, [refresh])

  async function run<T>(work: () => Promise<T>, failure: string): Promise<T | undefined> {
    if (busyRef.current) return
    generation.current += 1
    busyRef.current = true; setBusy(true); setError('')
    try {
      return await work()
    } catch (e) { if (mounted.current) { setError(e instanceof Error ? e.message : failure); toastError(e, failure) } }
    finally { busyRef.current = false; if (mounted.current) setBusy(false) }
  }
  async function reloadGroups() {
    const p = await api.singboxProxies().catch(() => null)
    if (p && mounted.current) setGroups(p.groups)
  }
  async function save(enabled: boolean, disableOnly = false) {
    await run(async () => {
      const s = await api.singboxSet(disableOnly && status ? { enabled: false, mode: status.mode, macs: status.macs, expected_revision: status.revision } : { enabled, mode, macs, expected_revision: editRevision.current })
      if (!mounted.current) return
      setStatus(s); setMode(s.mode); setMacs(s.macs); editRevision.current = s.revision; editing.current = false
      toast(enabled ? 'sing-box is starting. Selected devices use the proxy once it is ready.' : 'Saved. All devices use the normal network.')
      void reloadGroups()
    }, 'sing-box update failed')
  }
  async function changeProfile(action: SingBoxProfileAction, expectedRevision?: number) {
    if (!status) return
    if (action.action === 'activate' && status.enabled && !(await confirm({title:'Switch sing-box profile?',body:'sing-box restarts with the selected configuration. Proxied traffic briefly pauses; device routing choices are preserved.',confirmLabel:'Switch & apply'}))) return
    if (action.action === 'delete' && !(await confirm({title:'Delete saved profile?',body:'Its configuration and subscription link will be removed from the router.',confirmLabel:'Delete',danger:true}))) return
    return run(async () => {
      const s = await api.singboxSet({expected_revision: expectedRevision ?? (editing.current ? editRevision.current : status.revision), profile: action})
      if (!mounted.current) return
      setStatus(s); editRevision.current = s.revision
      // Partial profile actions never discard unsaved device choices.
      if (!editing.current) { setMode(s.mode); setMacs(s.macs) }
      if (action.action === 'save' || action.action === 'subscribe') { setConfig(''); setUrl(''); setProfileName(''); setProfileId(String(s.profiles[s.profiles.length - 1].id)) }
      if (action.action === 'delete') setProfileId('')
      toast(action.action === 'activate' ? (s.enabled ? 'Profile applied.' : 'Profile selected. sing-box remains off.') : action.action === 'refresh' ? 'Subscription updated.' : 'Saved profiles updated.')
      void reloadGroups()
      return true
    }, 'Profile update failed')
  }
  async function importFile(file?: File) {
    if (!file) return
    if (file.size > 524288) { toast('Configuration must be under 512 KiB', 'err'); return }
    const current = generation.current
    try {
      const text = await file.text()
      if (!mounted.current || busyRef.current || current !== generation.current) return
      setConfig(text); setProfileName(file.name.replace(/\.(json|txt)$/i, '').slice(0, 48))
    } catch (e) { toastError(e, 'Unable to read configuration') }
  }
  async function selectNode(group: string, name: string) {
    if (nodeBusy) return
    setNodeBusy(`select:${group}`)
    try {
      const p = await api.singboxSelect(group, name)
      if (mounted.current) { setGroups(p.groups); toast(`${group}: ${name}. Reconnect apps to move existing connections.`) }
    } catch (e) { toastError(e, 'Node selection failed') }
    finally { if (mounted.current) setNodeBusy('') }
  }
  async function testGroup(group: string) {
    if (nodeBusy) return
    setNodeBusy(`delay:${group}`)
    try {
      const p = await api.singboxDelay(group)
      if (mounted.current) setGroups(p.groups)
    } catch (e) { toastError(e, 'Latency test failed') }
    finally { if (mounted.current) setNodeBusy('') }
  }
  async function loadLogs() {
    try { const lines = await api.singboxLogs(); if (mounted.current) setLogs(lines) }
    catch (e) { toastError(e, 'Unable to load logs') }
  }

  const profiles = status?.profiles ?? []
  const max = status?.max_profiles ?? 5
  const picked = profiles.find(p => String(p.id) === profileId) ?? profiles.find(p => p.id === status?.active_profile) ?? profiles[0]
  const active = profiles.find(p => p.id === status?.active_profile)
  const dirty = mode !== status?.mode || [...macs].sort().join() !== [...(status?.macs ?? [])].sort().join()
  const ready = !!status?.binary_installed && !!status?.kernel_supported && !!status?.has_config && !status?.wireguard_enabled
  const summary = status?.enabled ? status.healthy ? 'Running' : status.error ? 'Needs attention' : 'Starting' : 'Off'
  const full = profiles.length >= max
  const canAdd = !!status && !full && !!profileName.trim() && (source === 'file' ? !!config.trim() : !!url.trim())

  return (
    <div className="space-y-4">
      <Card title="sing-box" action={<Chip tone={status?.enabled ? status.healthy ? 'ok' : 'warn' : 'default'}>{summary}</Chip>}>
        <div className="space-y-4">
          <div className="flex items-start justify-between gap-4">
            <div>
              <p className="text-[13px] font-medium text-ink">sing-box proxy for your connected devices</p>
              <p className="mt-1 text-xs text-ink2">Route every device automatically, or choose individual devices below. Routing rules from the imported configuration decide which traffic uses a proxy node.</p>
            </div>
            <Toggle label="Enable sing-box" checked={status?.enabled ?? false} disabled={busy || !status || (!status.enabled && !ready)} onChange={v => void save(v, !v)} />
          </div>
          {(error || status?.error) && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{error || status?.error}</p>}
          {status?.wireguard_enabled && !status.enabled && <p className="rounded-lg bg-warn/10 p-3 text-[13px] text-warn">WireGuard is on. Turn WireGuard off before enabling sing-box; only one can route devices at a time.</p>}
          {!status && <p className="text-sm text-ink2">Loading sing-box status…</p>}
          {status && <>
            <div className="flex flex-wrap gap-2">
              <Chip tone={status.binary_installed ? 'ok' : 'warn'}>sing-box {status.binary_installed ? status.version ?? 'installed' : 'not installed'}</Chip>
              <Chip tone={status.kernel_supported ? 'ok' : 'warn'}>TUN + REDIRECT {status.kernel_supported ? 'ready' : 'unavailable'}</Chip>
              {dirty && <Chip tone="warn">Unsaved changes</Chip>}
            </div>
            <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
              <Stat label="Version" value={status.version ?? (status.binary_installed ? 'Unknown' : 'Not installed')} />
              <Stat label="Downloaded" value={bytes(status.rx_bytes)} />
              <Stat label="Uploaded" value={bytes(status.tx_bytes)} />
              <Stat label="Memory" value={status.running ? bytes(status.memory_rss) : '—'} />
            </div>
            <p className="break-all text-xs text-ink2">{status.running ? `Running for ${duration(status.uptime)} · PID ${status.pid}` : 'Not running'}{active ? ` · Profile ${active.name}` : ''}</p>
            {status.starting && <p className="rounded-lg bg-surface2/60 p-3 text-xs text-ink2">Starting: sing-box downloads remote rule sets before it accepts traffic, which can take a minute on the first start. Selected devices are blocked until it is ready.</p>}
          </>}
        </div>
      </Card>

      <Card title="Node selection" action={groups.length > 0 ? <Chip tone="ok">{groups.length} group{groups.length === 1 ? '' : 's'}</Chip> : undefined}>
        <div className="space-y-4">
          {!status?.running ? <p className="text-sm text-ink2">Enable sing-box to choose nodes. Choices are remembered for each profile.</p>
            : groups.length === 0 ? <p className="text-sm text-ink2">No selector groups found in this configuration.</p>
            : groups.map(g => (
              <div key={g.name} className="space-y-2">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="min-w-0">
                    <p className="truncate text-[13px] font-semibold text-ink">{g.name}</p>
                    <p className="truncate text-xs text-ink3">{g.selectable ? 'Manual selection' : `${g.type} · automatic`} · Current: {g.now ?? '—'}</p>
                  </div>
                  <Button size="sm" variant="outline" loading={nodeBusy === `delay:${g.name}`} disabled={!!nodeBusy} onClick={() => void testGroup(g.name)}>Test latency</Button>
                </div>
                <div className="grid max-h-80 grid-cols-1 gap-2 overflow-y-auto sm:grid-cols-2 lg:grid-cols-3">
                  {g.options.map(o => {
                    const current = o.name === g.now, l = latency(o.delay)
                    return <button key={o.name} type="button" disabled={!g.selectable || !!nodeBusy || current} onClick={() => void selectNode(g.name, o.name)}
                      className={`flex min-w-0 items-center justify-between gap-2 rounded-lg border px-3 py-2 text-left transition-colors ${current ? 'border-accent bg-accent/10' : 'border-line/10 hover:bg-surface2/60'} ${g.selectable ? '' : 'cursor-default'}`}>
                      <span className="min-w-0"><span className={`block truncate text-[13px] font-medium ${current ? 'text-accent' : 'text-ink'}`}>{o.name}</span><span className="block truncate text-[11px] text-ink3">{o.type ?? 'Unknown'}</span></span>
                      <span className={`shrink-0 text-[11px] font-semibold tnum ${l.className}`}>{l.text}</span>
                    </button>
                  })}
                </div>
              </div>
            ))}
          {status?.running && groups.length > 0 && <p className="text-xs text-ink3">Switching takes effect immediately without restarting sing-box. Existing connections may keep the previous node until they reconnect.</p>}
        </div>
      </Card>

      <Card title="Saved configurations" action={<Chip>{profiles.length} / {max}</Chip>}>
        <div className="space-y-3">
          <p className="text-xs text-ink2">Current profile: <strong className="text-ink">{active?.name ?? 'None selected'}</strong>. Device routing choices apply to every profile.</p>
          {profiles.length > 0 ? <>
            <Select aria-label="Saved sing-box configuration" value={picked ? String(picked.id) : ''} disabled={busy} onChange={e => { setProfileId(e.target.value); setRenameDraft(null) }}>
              {profiles.map(p => <option key={p.id} value={p.id}>{p.name}{p.id === status?.active_profile ? ' (current)' : ''}</option>)}
            </Select>
            {picked && <p className="break-all text-xs text-ink3">{picked.source === 'subscription' ? `Subscription · ${picked.host ?? ''}` : 'Imported file'} · {picked.nodes} node{picked.nodes === 1 ? '' : 's'} · updated {ago(picked.updated)}</p>}
            <div className="flex flex-wrap gap-2">
              <Button variant="primary" disabled={busy || !picked || picked.id === status?.active_profile} onClick={() => picked && void changeProfile({action:'activate',id:picked.id})}>{status?.enabled ? 'Switch & apply' : 'Use configuration'}</Button>
              {picked?.source === 'subscription' && <Button variant="outline" disabled={busy} onClick={() => void changeProfile({action:'refresh',id:picked.id})}>Update subscription</Button>}
              <Button variant="outline" disabled={busy || !picked} onClick={() => picked && status && setRenameDraft({id:picked.id,name:picked.name,revision:status.revision})}>Rename</Button>
              <Button variant="danger" disabled={busy || !picked || (!!status?.enabled && picked.id === status.active_profile)} onClick={() => picked && void changeProfile({action:'delete',id:picked.id})}>Delete</Button>
            </div>
            {renameDraft !== null && <div className="flex flex-wrap items-center gap-2">
              <Input aria-label="Rename profile" value={renameDraft.name} maxLength={48} disabled={busy} onChange={e => setRenameDraft({...renameDraft,name:e.target.value})} className="min-w-0 flex-1" />
              <Button disabled={busy || !renameDraft.name.trim()} onClick={() => { void changeProfile({action:'rename',id:renameDraft.id,name:renameDraft.name}, renameDraft.revision).then(ok => { if (ok) setRenameDraft(null) }) }}>Save name</Button>
              <Button variant="ghost" disabled={busy} onClick={() => setRenameDraft(null)}>Cancel</Button>
            </div>}
            <p className="text-xs text-ink3">Switching keeps sing-box on or off as it is. Updating the current subscription while sing-box is on restarts it with the new configuration.</p>
          </> : <p className="text-sm text-ink2">No saved configurations. Import your first profile below.</p>}
        </div>
      </Card>

      <Card title="Add configuration">
        <div className="space-y-3">
          <Segmented value={source} disabled={busy || full} onChange={setSource} options={[{value:'file',label:'JSON file or text'},{value:'subscription',label:'Subscription URL'}]} />
          <label className="block text-xs font-medium text-ink2">Profile name<Input aria-label="New profile name" value={profileName} maxLength={48} disabled={busy || full} onChange={e => setProfileName(e.target.value)} placeholder="e.g. My proxy" className="mt-1" /></label>
          {source === 'file' ? <>
            <p className="text-xs leading-relaxed text-ink2">Import a complete sing-box JSON configuration (for sing-box {status?.version ?? '1.14'}). Its inbounds are replaced by the router’s own: a redirect listener for TCP and a TUN interface for UDP. Outbounds, DNS and routing rules are kept. Local file paths, routing marks and Tor are not accepted.</p>
            <input aria-label="Import sing-box configuration file" type="file" accept=".json,.txt,application/json" disabled={busy || full} className="block w-full text-xs text-ink2 file:mr-3 file:rounded-lg file:border-0 file:bg-surface2 file:px-3 file:py-2 file:text-ink" onChange={e => { void importFile(e.target.files?.[0]); e.target.value = '' }} />
            <textarea aria-label="sing-box configuration" spellCheck={false} autoComplete="off" disabled={busy || full} value={config} onChange={e => setConfig(e.target.value)} rows={8} placeholder={'{\n  "outbounds": [\n    { "type": "vless", "tag": "HK 01", "server": "…", … },\n    { "type": "direct", "tag": "direct" }\n  ],\n  "route": { "rules": [ … ] }\n}'} className="w-full resize-y rounded-lg border border-line/15 bg-surface2/50 p-3 font-mono text-xs text-ink outline-none focus:border-accent" />
          </> : <>
            <p className="text-xs leading-relaxed text-ink2">The router downloads the link with its own connection. The link must return sing-box JSON; share links and Clash YAML subscriptions are not supported. The full link is stored on the router and only its host is shown again.</p>
            <Input aria-label="Subscription URL" type="url" value={url} disabled={busy || full} onChange={e => setUrl(e.target.value)} placeholder="https://example.com/subscription?format=sing-box" autoComplete="off" spellCheck={false} />
          </>}
          <Button variant="primary" loading={busy} disabled={!canAdd} onClick={() => void changeProfile(source === 'file' ? {action:'save',name:profileName,config_text:config} : {action:'subscribe',name:profileName,url})}>{source === 'file' ? 'Save new profile' : 'Download & save'}</Button>
          <p className="text-xs text-ink2">{full ? `All ${max} slots are in use. Delete a profile before adding another.` : `Up to ${max} profiles. Saving does not switch the current configuration. The first profile is selected automatically, with sing-box still off.`}</p>
        </div>
      </Card>

      <Card title="Which devices use sing-box?">
        <div className="space-y-3">
          <DeviceRouting feature="sing-box" mode={mode} macs={macs} clients={clients} disabled={busy} onChange={(nextMode, nextMacs) => { editing.current = true; setMode(nextMode); setMacs(nextMacs) }} />
          <p className="rounded-lg bg-surface2/60 p-3 text-xs leading-relaxed text-ink2">When enabled, proxied devices cannot fall back to the normal network if sing-box stops; the router restarts it automatically. TCP is redirected to sing-box and UDP is routed through its TUN; IPv4 DNS from proxied devices is answered by sing-box. IPv6 internet is blocked for proxied devices to prevent bypass. Router management stays accessible. Applying changes briefly pauses forwarded traffic while rules are replaced.</p>
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" loading={busy} disabled={!status || !status.has_config} onClick={() => void save(status?.enabled ?? false)}>Save {status?.enabled ? '& apply' : 'device choices'}</Button>
            {!status?.enabled && <Button variant="outline" disabled={busy || !ready} onClick={() => void save(true)}>Save & enable</Button>}
            <Button variant="ghost" disabled={busy} onClick={() => { if ((!editing.current && !config && !url && !profileName && renameDraft === null) || window.confirm('Discard unsaved changes and reload the latest settings?')) { editing.current = false; setConfig(''); setUrl(''); setProfileName(''); setRenameDraft(null); void refresh() } }}>Refresh</Button>
          </div>
        </div>
      </Card>

      <Card title="Logs" action={<div className="flex gap-2">{logs !== null && <Button size="sm" variant="ghost" onClick={() => void loadLogs()}>Refresh</Button>}<Button size="sm" variant="outline" onClick={() => logs === null ? void loadLogs() : setLogs(null)}>{logs === null ? 'Show' : 'Hide'}</Button></div>}>
        {logs === null ? <p className="text-xs text-ink3">Recent sing-box warnings and errors, kept in memory on the router.</p>
          : logs.length === 0 ? <p className="text-sm text-ink2">No log entries yet.</p>
          : <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-surface2/60 p-3 font-mono text-[11px] leading-relaxed text-ink2">{logs.join('\n')}</pre>}
      </Card>
    </div>
  )
}
