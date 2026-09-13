import WireGuardTab from './WireGuardTab'

export default function WireGuardPage() {
  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-xl font-bold text-ink">WireGuard</h1>
        <p className="mt-0.5 text-[13px] text-ink2">VPN profiles, tunnel status and device routing</p>
      </div>
      <WireGuardTab />
    </div>
  )
}
