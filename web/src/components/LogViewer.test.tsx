import { describe, expect, it, vi } from 'vitest'
import { act, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { LogViewer } from './LogViewer'
import type { LogLine } from '../api/client'

const line = (order: number, extra: Partial<LogLine> = {}): LogLine => ({
  order,
  kind: 'stdout',
  content: `line ${order}`,
  hidden: false,
  batch: 1,
  timestamp: new Date(order * 1000).toISOString(),
  ...extra,
})

const ords = () =>
  screen.getAllByTestId('log-line').map((el) => el.getAttribute('data-ord'))

describe('LogViewer', () => {
  it('renders fetched lines then appends ws lines in ord order', async () => {
    let push: (l: LogLine) => void = () => {}
    const fetchLines = vi.fn().mockResolvedValue([line(1), line(2)])
    const subscribe = vi.fn((cb: (l: LogLine) => void) => {
      push = cb
      return () => {}
    })

    render(<LogViewer fetchLines={fetchLines} subscribe={subscribe} />)
    await screen.findByText('line 1')

    // ws lines arrive out of order
    act(() => {
      push(line(4))
      push(line(3))
    })

    expect(ords()).toEqual(['1', '2', '3', '4'])
  })

  it('dedupes lines already seen when refetching', async () => {
    let push: (l: LogLine) => void = () => {}
    const fetchLines = vi
      .fn()
      .mockResolvedValueOnce([line(1), line(2)])
      .mockResolvedValueOnce([line(1), line(2), line(3), line(4), line(5)])
    const subscribe = vi.fn((cb: (l: LogLine) => void) => {
      push = cb
      return () => {}
    })

    const { rerender } = render(
      <LogViewer fetchLines={fetchLines} subscribe={subscribe} refreshKey={0} />,
    )
    await screen.findByText('line 2')

    act(() => {
      push(line(3))
      push(line(4))
    })

    // refetch overlaps with everything already displayed
    rerender(<LogViewer fetchLines={fetchLines} subscribe={subscribe} refreshKey={1} />)
    await screen.findByText('line 5')

    expect(ords()).toEqual(['1', '2', '3', '4', '5'])
    expect(screen.getAllByText('line 3')).toHaveLength(1)
  })

  it('ignores duplicate ws deliveries of the same ord', async () => {
    let push: (l: LogLine) => void = () => {}
    const fetchLines = vi.fn().mockResolvedValue([line(1)])
    const subscribe = vi.fn((cb: (l: LogLine) => void) => {
      push = cb
      return () => {}
    })

    render(<LogViewer fetchLines={fetchLines} subscribe={subscribe} />)
    await screen.findByText('line 1')

    act(() => {
      push(line(2))
      push(line(2))
      push(line(1))
    })

    expect(ords()).toEqual(['1', '2'])
  })

  it('colors stderr lines red', async () => {
    const fetchLines = vi
      .fn()
      .mockResolvedValue([line(1), line(2, { kind: 'stderr', content: 'oh no' })])

    render(<LogViewer fetchLines={fetchLines} />)
    const row = (await screen.findByText('oh no')).closest('[data-testid="log-line"]')

    expect(row).toHaveAttribute('data-kind', 'stderr')
    expect(row?.className).toMatch(/red/)
  })

  it('hides hidden lines until toggled on', async () => {
    const fetchLines = vi
      .fn()
      .mockResolvedValue([line(1), line(2, { hidden: true, content: 'secret setup' })])

    render(<LogViewer fetchLines={fetchLines} />)
    await screen.findByText('line 1')

    expect(screen.queryByText('secret setup')).not.toBeInTheDocument()

    const user = userEvent.setup()
    await user.click(screen.getByLabelText(/show hidden/i))
    expect(screen.getByText('secret setup')).toBeInTheDocument()

    await user.click(screen.getByLabelText(/show hidden/i))
    expect(screen.queryByText('secret setup')).not.toBeInTheDocument()
  })
})
