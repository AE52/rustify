import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MetricsPanel } from './MetricsPanel'
import { renderApp } from '../test/harness'

/** A fetch stub that matches the metric by pathname, ignoring the `from` query. */
function stubMetrics(byMetric: Record<string, [number, number][]>) {
  const fetchMock = vi.fn(async (url: string) => {
    const m = /\/metrics\/(\w+)/.exec(String(url))
    const data = m ? (byMetric[m[1]] ?? []) : []
    return new Response(JSON.stringify(data), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

afterEach(() => vi.unstubAllGlobals())

describe('<MetricsPanel />', () => {
  it('plots [ts, value] series and labels the latest sample', async () => {
    stubMetrics({
      cpu: [
        [1000, 10],
        [2000, 90],
      ],
      memory: [[3000, 1024 * 1024]],
    })
    // High refresh so polling doesn't interfere with call-count assertions.
    renderApp(
      <MetricsPanel
        resource="servers"
        uuid="s1"
        refreshMs={10_000_000}
        specs={[
          { metric: 'cpu', label: 'CPU', percent: true },
          { metric: 'memory', label: 'Memory' },
        ]}
      />,
    )

    // Percentage series renders its latest value; byte series is humanised.
    expect(await screen.findByText('90.0%')).toBeInTheDocument()
    expect(await screen.findByText('1.0 MiB')).toBeInTheDocument()
    // A polyline is drawn for the CPU chart.
    const svg = screen.getByLabelText('CPU chart')
    expect(svg.querySelector('polyline')).toBeTruthy()
  })

  it('refetches with a new window when the range is switched', async () => {
    const fetchMock = stubMetrics({ cpu: [[1000, 5]] })
    const user = userEvent.setup()
    renderApp(
      <MetricsPanel
        resource="servers"
        uuid="s1"
        refreshMs={10_000_000}
        specs={[{ metric: 'cpu', label: 'CPU', percent: true }]}
      />,
    )

    await screen.findByText('5.0%')
    const cpuCalls = () =>
      fetchMock.mock.calls.filter(([url]) => String(url).includes('/metrics/cpu'))
    const before = cpuCalls().length
    const firstFrom = new URL(String(cpuCalls()[0][0]), 'http://x').searchParams.get('from')

    await user.click(screen.getByRole('button', { name: '1h' }))

    await waitFor(() => expect(cpuCalls().length).toBeGreaterThan(before))
    const lastFrom = new URL(
      String(cpuCalls()[cpuCalls().length - 1][0]),
      'http://x',
    ).searchParams.get('from')
    expect(lastFrom).not.toBe(firstFrom)
  })
})
