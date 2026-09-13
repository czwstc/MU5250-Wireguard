import { useEffect, useState } from 'react'
import { api } from '../../data/api'
import type { ScreenStatus } from '../../types'
import { Button } from '../../ui/controls'
import { Card, Chip } from '../../ui/primitives'

const labels: Record<string, string> = { idle: 'Stock UI', starting: 'Opening', active: 'Panel active', restoring: 'Restoring', error: 'Check recovery status' }
export default function ScreenControl() {
  const [status, setStatus] = useState<ScreenStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [loadError, setLoadError] = useState('')
  useEffect(() => {
    let alive = true, reading = false
    const refresh = async () => {
      if (reading || document.hidden) return
      reading = true
      try { const s = await api.screen(); if (alive) { setStatus(s); setLoadError('') } }
      catch (e) { if (alive) setLoadError(e instanceof Error ? e.message : 'Unable to read screen status') }
      finally { reading = false }
    }
    void refresh()
    const timer = window.setInterval(() => void refresh(), 2000)
    return () => { alive = false; window.clearInterval(timer) }
  }, [])
  async function change(open: boolean) {
    setBusy(true); setError('')
    try {
      await (open ? api.screenOpen() : api.screenClose())
      setStatus(s => s ? { ...s, running: true, state: open ? 'starting' : 'restoring' } : s)
    } catch (e) { setError(e instanceof Error ? e.message : 'Screen action failed') }
    finally { setBusy(false) }
  }
  return <Card title="Device screen" action={<Chip tone={status?.state === 'error' ? 'warn' : status?.running ? 'accent' : 'default'}>{status ? labels[status.state] ?? status.state : 'Loading'}</Chip>}>
    <div className="space-y-3">
      <p className="text-xs leading-relaxed text-ink2">View WireGuard status, control the tunnel and select devices on the touchscreen. Exit or leave it idle for two minutes to restore the stock UI. Saved routing rules stay active.</p>
      {(error || loadError) && <p role="alert" className="text-xs text-danger">{error || loadError}</p>}
      {status && (!status.available || !status.verified) && <p className="text-xs text-ink2">The screen component is not installed or has not passed recovery verification on this device.</p>}
      <div className="flex flex-wrap gap-2">
        <Button variant="primary" disabled={busy || !status?.available || !status.verified || status.running} onClick={() => void change(true)}>Open device screen</Button>
        <Button variant="outline" disabled={busy || !status?.available || (!status.running && status.state !== 'error')} onClick={() => void change(false)}>Restore stock UI</Button>
      </div>
    </div>
  </Card>
}
