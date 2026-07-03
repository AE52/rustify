import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { api, type MetricPoint } from '../api/client'
import { METRIC_WINDOWS, formatBytes, formatPercent, windowFrom, type MetricWindow } from '../lib/metrics'
import { MetricsChart } from './MetricsChart'
import { ErrorNote } from './ui'

export interface MetricSpec {
  metric: string
  label: string
  /** Percentage series render on a fixed 0..100 axis. */
  percent?: boolean
}

export interface MetricsPanelProps {
  /** `servers` (host metrics) or `containers` (per-container metrics). */
  resource: 'servers' | 'containers'
  uuid: string
  specs: MetricSpec[]
  /** Poll cadence; defaults to 5s (contract C5). */
  refreshMs?: number
}

function MetricSeries({
  resource,
  uuid,
  spec,
  window,
  refreshMs,
}: {
  resource: 'servers' | 'containers'
  uuid: string
  spec: MetricSpec
  window: MetricWindow
  refreshMs: number
}) {
  const query = useQuery({
    queryKey: ['metrics', resource, uuid, spec.metric, window],
    queryFn: () =>
      api.get<MetricPoint[]>(
        `/${resource}/${uuid}/metrics/${spec.metric}?from=${encodeURIComponent(windowFrom(window))}`,
      ),
    refetchInterval: refreshMs,
  })

  if (query.isError) return <ErrorNote error={query.error} />

  return (
    <MetricsChart
      data={query.data ?? []}
      percent={spec.percent}
      label={spec.label}
      format={spec.percent ? formatPercent : formatBytes}
    />
  )
}

/**
 * A metrics panel: a window selector (10m / 1h / 24h) over one or more line
 * charts, each polling `GET /{resource}/{uuid}/metrics/{metric}?from=` on a 5s
 * interval. Switching the window re-derives the `from` bound and refetches.
 */
export function MetricsPanel({ resource, uuid, specs, refreshMs = 5_000 }: MetricsPanelProps) {
  const [window, setWindow] = useState<MetricWindow>('10m')

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-1" role="group" aria-label="metrics window">
        {METRIC_WINDOWS.map((w) => (
          <button
            key={w.id}
            type="button"
            aria-pressed={window === w.id}
            onClick={() => setWindow(w.id)}
            className={`rounded-md px-2.5 py-1 text-xs ${
              window === w.id
                ? 'bg-zinc-100 font-semibold text-zinc-900'
                : 'border border-zinc-700 text-zinc-400 hover:bg-zinc-800'
            }`}
          >
            {w.label}
          </button>
        ))}
      </div>
      <div className="grid gap-4 sm:grid-cols-2">
        {specs.map((spec) => (
          <MetricSeries
            key={spec.metric}
            resource={resource}
            uuid={uuid}
            spec={spec}
            window={window}
            refreshMs={refreshMs}
          />
        ))}
      </div>
    </div>
  )
}
