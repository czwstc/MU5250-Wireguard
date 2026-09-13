import { useState } from 'react'
import { Tabs } from '../../ui/Tabs'
import Overview from './Overview'
import Locking from './Locking'

type Tab = 'overview' | 'locking'

export default function SignalGroup() {
  const [tab, setTab] = useState<Tab>('overview')

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-xl font-bold text-ink">Signal</h1>
        <p className="mt-0.5 text-[13px] text-ink2">Live radio metrics, band and cell locking</p>
      </div>

      <Tabs
        tabs={[
          { id: 'overview', label: 'Overview' },
          { id: 'locking', label: 'Mode & Locking' },
        ]}
        active={tab}
        onChange={setTab}
      />

      {tab === 'overview' ? <Overview /> : <Locking />}
    </div>
  )
}
