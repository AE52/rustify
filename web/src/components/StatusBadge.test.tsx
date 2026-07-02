import { describe, expect, it } from 'vitest'
import { render, screen } from '@testing-library/react'
import { StatusBadge, statusColor } from './StatusBadge'

describe('StatusBadge', () => {
  it('maps running:healthy to green', () => {
    render(<StatusBadge status="running:healthy" />)
    expect(screen.getByTestId('status-badge')).toHaveAttribute('data-color', 'green')
  })

  it('maps exited to gray', () => {
    render(<StatusBadge status="exited" />)
    expect(screen.getByTestId('status-badge')).toHaveAttribute('data-color', 'gray')
  })

  it('maps crashed to red', () => {
    render(<StatusBadge status="crashed" />)
    expect(screen.getByTestId('status-badge')).toHaveAttribute('data-color', 'red')
  })

  it('maps deployment statuses', () => {
    expect(statusColor('queued')).toBe('blue')
    expect(statusColor('in_progress')).toBe('yellow')
    expect(statusColor('finished')).toBe('green')
    expect(statusColor('failed')).toBe('red')
    expect(statusColor('cancelled')).toBe('gray')
    expect(statusColor('running:unhealthy')).toBe('yellow')
  })
})
