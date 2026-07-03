import { describe, expect, it } from 'vitest'
import {
  chartModel,
  formatBytes,
  formatPercent,
  niceMax,
  windowFrom,
  windowMs,
} from './metrics'
import type { MetricPoint } from '../api/client'

describe('windowFrom', () => {
  it('subtracts the window span and emits ISO-8601 Zulu', () => {
    const now = Date.parse('2026-07-03T12:00:00Z')
    expect(windowFrom('10m', now)).toBe('2026-07-03T11:50:00.000Z')
    expect(windowFrom('1h', now)).toBe('2026-07-03T11:00:00.000Z')
    expect(windowFrom('24h', now)).toBe('2026-07-02T12:00:00.000Z')
  })

  it('exposes window spans in ms', () => {
    expect(windowMs('10m')).toBe(600_000)
    expect(windowMs('1h')).toBe(3_600_000)
  })
})

describe('niceMax', () => {
  it('rounds up to a friendly bound', () => {
    expect(niceMax(42)).toBe(50)
    expect(niceMax(7)).toBe(10)
    expect(niceMax(0)).toBe(1)
    expect(niceMax(120)).toBe(200)
  })
})

describe('chartModel', () => {
  const geom = { width: 100, height: 100, pad: 10, yMin: 0, yMax: 100 }

  it('returns empty points for no data', () => {
    const m = chartModel([], geom)
    expect(m.points).toBe('')
    expect(m.last).toBeNull()
  })

  it('maps endpoints to the axis extremes', () => {
    const data: MetricPoint[] = [
      [1000, 0],
      [2000, 100],
    ]
    const m = chartModel(data, geom)
    const coords = m.points.split(' ').map((p) => p.split(',').map(Number))
    // first sample: left edge (x=pad), value 0 -> bottom (y=height-pad)
    expect(coords[0][0]).toBeCloseTo(10)
    expect(coords[0][1]).toBeCloseTo(90)
    // last sample: right edge (x=width-pad), value 100 -> top (y=pad)
    expect(coords[1][0]).toBeCloseTo(90)
    expect(coords[1][1]).toBeCloseTo(10)
    expect(m.last).toBe(100)
  })

  it('autoscales the value axis when bounds are omitted', () => {
    const data: MetricPoint[] = [
      [1000, 12],
      [2000, 42],
    ]
    const m = chartModel(data, { width: 100, height: 100, pad: 0 })
    expect(m.yMin).toBe(0)
    expect(m.yMax).toBe(50)
  })
})

describe('formatting', () => {
  it('formats bytes', () => {
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(1024)).toBe('1.0 KiB')
    expect(formatBytes(1024 * 1024 * 3)).toBe('3.0 MiB')
  })

  it('formats percentages', () => {
    expect(formatPercent(42.345)).toBe('42.3%')
  })
})
