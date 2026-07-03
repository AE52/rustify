import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { ScheduledTasks } from './ScheduledTasks'
import { isValidFrequency } from '../lib/cron'
import { mockFetch, renderApp } from '../test/harness'

describe('cron frequency validation', () => {
  it('accepts aliases and valid 5-field crons, rejects garbage', () => {
    expect(isValidFrequency('daily')).toBe(true)
    expect(isValidFrequency('@weekly')).toBe(true)
    expect(isValidFrequency('0 3 * * *')).toBe(true)
    expect(isValidFrequency('*/5 * * * *')).toBe(true)
    expect(isValidFrequency('not a cron')).toBe(false)
    expect(isValidFrequency('0 3 * *')).toBe(false)
    expect(isValidFrequency('')).toBe(false)
  })
})

describe('ScheduledTasks create form', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('blocks submit on an invalid frequency and enables on a valid one', async () => {
    mockFetch({ 'GET /applications/a1/scheduled-tasks': [] })
    const user = userEvent.setup()
    renderApp(<ScheduledTasks resource="applications" uuid="a1" />)

    await user.type(screen.getByPlaceholderText('db-cleanup'), 'cleanup')
    await user.type(screen.getByPlaceholderText('php artisan schedule:run'), 'echo hi')

    const freq = screen.getByPlaceholderText('daily or 0 3 * * *')
    await user.clear(freq)
    await user.type(freq, 'every other tuesday')

    const submit = screen.getByRole('button', { name: /create task/i })
    expect(submit).toBeDisabled()
    expect(screen.getByText(/not a valid cron/i)).toBeInTheDocument()

    await user.clear(freq)
    await user.type(freq, '0 3 * * *')
    await waitFor(() => expect(submit).toBeEnabled())
  })

  it('posts a valid task to the resource endpoint', async () => {
    const fetchMock = mockFetch({ 'GET /applications/a1/scheduled-tasks': [] })
    const user = userEvent.setup()
    renderApp(<ScheduledTasks resource="applications" uuid="a1" />)

    await user.type(screen.getByPlaceholderText('db-cleanup'), 'cleanup')
    await user.type(screen.getByPlaceholderText('php artisan schedule:run'), 'echo hi')
    await user.click(screen.getByRole('button', { name: /create task/i }))

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) =>
          url === '/api/v1/applications/a1/scheduled-tasks' && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
      expect(JSON.parse(String(call?.[1]?.body)).command).toBe('echo hi')
    })
  })
})
