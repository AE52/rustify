import { useApplication } from './index'
import { MetricsPanel } from '../../../components/MetricsPanel'
import { SectionTitle } from '../../../components/ui'

/** Application metrics tab: per-container CPU (%) and memory (bytes). */
export default function ApplicationMetrics() {
  const { app } = useApplication()
  return (
    <div className="flex flex-col gap-4">
      <SectionTitle>Metrics</SectionTitle>
      <MetricsPanel
        resource="containers"
        uuid={app.uuid}
        specs={[
          { metric: 'cpu', label: 'CPU', percent: true },
          { metric: 'memory', label: 'Memory' },
        ]}
      />
    </div>
  )
}
