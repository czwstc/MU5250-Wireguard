import { useState } from 'react'
import { Tabs } from '../../ui/Tabs'
import ClientsTab from './ClientsTab'
import WifiTab from './WifiTab'
import RouterTab from './RouterTab'

type Tab = 'clients' | 'wifi' | 'router'

export default function NetworkGroup() {
  const [tab, setTab] = useState<Tab>('clients')

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-xl font-bold text-ink">Network</h1>
        <p className="mt-0.5 text-[13px] text-ink2">Connected clients, Wi-Fi and router settings</p>
      </div>

      <Tabs
        tabs={[
          { id: 'clients', label: 'Clients' },
          { id: 'wifi', label: 'Wi-Fi' },
          { id: 'router', label: 'Router' },
        ]}
        active={tab}
        onChange={setTab}
      />

      {tab === 'clients' && <ClientsTab />}
      {tab === 'wifi' && <WifiTab />}
      {tab === 'router' && <RouterTab />}
    </div>
  )
}
