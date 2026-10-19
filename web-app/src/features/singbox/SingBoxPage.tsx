import SingBoxTab from './SingBoxTab'

export default function SingBoxPage() {
  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-xl font-bold text-ink">sing-box</h1>
        <p className="mt-0.5 text-[13px] text-ink2">Proxy configurations, node selection and device routing</p>
      </div>
      <SingBoxTab />
    </div>
  )
}
