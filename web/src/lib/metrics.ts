/**
 * Pure helpers for the metrics charts (contract C5). The server returns a JSON
 * array of `[unix_time_seconds, value]` pairs, oldest-first; these functions map
 * a window selection to the `from=` query and project points into an SVG
 * viewbox. The React `MetricsChart` composes them; the maths is unit-tested.
 */
import type { MetricPoint } from '../api/client'

export type MetricWindow = '10m' | '1h' | '24h'

export const METRIC_WINDOWS: { id: MetricWindow; label: string; ms: number }[] = [
  { id: '10m', label: '10m', ms: 10 * 60_000 },
  { id: '1h', label: '1h', ms: 60 * 60_000 },
  { id: '24h', label: '24h', ms: 24 * 60 * 60_000 },
]

/** Millisecond span of a window. */
export function windowMs(window: MetricWindow): number {
  return METRIC_WINDOWS.find((w) => w.id === window)?.ms ?? METRIC_WINDOWS[0].ms
}

/** ISO-8601 Zulu `from` timestamp for a window ending `now`. */
export function windowFrom(window: MetricWindow, nowMs: number = Date.now()): string {
  return new Date(nowMs - windowMs(window)).toISOString()
}

export interface ChartGeom {
  width: number
  height: number
  pad: number
  /** Fixed lower bound (e.g. 0 for a percentage). Omit to autoscale. */
  yMin?: number
  /** Fixed upper bound (e.g. 100 for a percentage). Omit to autoscale. */
  yMax?: number
}

export interface ChartModel {
  /** SVG polyline `points` string (`"x,y x,y …"`). Empty when no data. */
  points: string
  /** Resolved value axis bounds after autoscaling. */
  yMin: number
  yMax: number
  /** Latest sample value, or null when the series is empty. */
  last: number | null
}

/** Round a positive maximum up to a friendly axis bound. */
export function niceMax(max: number): number {
  if (max <= 0) return 1
  const mag = 10 ** Math.floor(Math.log10(max))
  const norm = max / mag
  const step = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10
  return step * mag
}

/**
 * Project a series into an SVG polyline. Time is scaled across the full x-axis
 * (first sample at the left edge, last at the right); values are scaled to the
 * y-axis with the origin at the bottom. A single point renders as a flat line.
 */
export function chartModel(data: MetricPoint[], geom: ChartGeom): ChartModel {
  const { width, height, pad } = geom
  if (data.length === 0) {
    return { points: '', yMin: geom.yMin ?? 0, yMax: geom.yMax ?? 1, last: null }
  }

  const values = data.map((p) => p[1])
  const yMin = geom.yMin ?? Math.min(0, ...values)
  const yMax = geom.yMax ?? niceMax(Math.max(...values))
  const span = yMax - yMin || 1

  const tMin = data[0][0]
  const tMax = data[data.length - 1][0]
  const tSpan = tMax - tMin || 1

  const x = (t: number) => pad + ((t - tMin) / tSpan) * (width - 2 * pad)
  const y = (v: number) => height - pad - ((v - yMin) / span) * (height - 2 * pad)

  const points = data
    .map((p) => `${x(p[0]).toFixed(2)},${y(p[1]).toFixed(2)}`)
    .join(' ')

  return { points, yMin, yMax, last: values[values.length - 1] }
}

/** Human-readable byte count for the container-memory axis. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${Math.round(bytes)} B`
  const units = ['KiB', 'MiB', 'GiB', 'TiB']
  let v = bytes / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i += 1
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`
}

/** One-decimal percentage. */
export function formatPercent(value: number): string {
  return `${value.toFixed(1)}%`
}
