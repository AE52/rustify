import { useMemo } from 'react'
import { chartModel, type ChartGeom } from '../lib/metrics'
import type { MetricPoint } from '../api/client'

const WIDTH = 480
const HEIGHT = 140
const PAD = 8

export interface MetricsChartProps {
  data: MetricPoint[]
  /** Fixed 0..100 axis for percentages; autoscaled when false. */
  percent?: boolean
  /** Formats the latest-value label and, when percent, drives the axis. */
  format: (value: number) => string
  label: string
}

/**
 * Lean inline-SVG line chart. Theme-aware: the stroke uses `currentColor`, set
 * by a Tailwind text colour on the wrapper, so it reads in light and dark. A
 * subtle fill under the line gives the series body.
 */
export function MetricsChart({ data, percent, format, label }: MetricsChartProps) {
  const model = useMemo(() => {
    const geom: ChartGeom = percent
      ? { width: WIDTH, height: HEIGHT, pad: PAD, yMin: 0, yMax: 100 }
      : { width: WIDTH, height: HEIGHT, pad: PAD }
    return chartModel(data, geom)
  }, [data, percent])

  const area =
    model.points.length > 0
      ? `${PAD},${HEIGHT - PAD} ${model.points} ${WIDTH - PAD},${HEIGHT - PAD}`
      : ''

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline justify-between">
        <span className="text-xs font-medium text-zinc-400">{label}</span>
        <span data-testid="metric-last" className="font-mono text-sm text-zinc-100">
          {model.last === null ? '—' : format(model.last)}
        </span>
      </div>
      <div className="rounded-lg border border-zinc-800 bg-zinc-950 p-2 text-sky-400">
        <svg
          role="img"
          aria-label={`${label} chart`}
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          preserveAspectRatio="none"
          className="h-32 w-full"
        >
          {model.points.length === 0 ? (
            <text
              x={WIDTH / 2}
              y={HEIGHT / 2}
              textAnchor="middle"
              className="fill-zinc-600 text-[11px]"
            >
              no samples yet
            </text>
          ) : (
            <>
              <polygon points={area} className="fill-sky-500/10" />
              <polyline
                points={model.points}
                fill="none"
                stroke="currentColor"
                strokeWidth={1.5}
                strokeLinejoin="round"
                strokeLinecap="round"
                vectorEffect="non-scaling-stroke"
              />
            </>
          )}
        </svg>
      </div>
    </div>
  )
}
