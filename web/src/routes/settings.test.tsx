import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import Settings from './settings'
import { mockFetch, renderApp } from '../test/harness'

const baseRoutes = {
  'GET /auth/me': { id: 'u1', email: 'a@b.c', name: 'A', team_uuid: 't1' },
  'GET /settings': { fqdn: null, wildcard_domain: null, registration_enabled: false },
  'GET /api-tokens': [],
}

describe('S3 storage form (settings)', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('requires name, bucket, key and secret before submit', async () => {
    mockFetch({ ...baseRoutes, 'GET /s3-storages': [] })
    const user = userEvent.setup()
    renderApp(<Settings />)

    const submit = await screen.findByRole('button', { name: /add s3 storage/i })
    expect(submit).toBeDisabled()

    await user.type(screen.getByPlaceholderText('backups'), 'my-s3')
    await user.type(screen.getByPlaceholderText('my-bucket'), 'bkt')
    expect(submit).toBeDisabled()

    await user.type(screen.getByLabelText('Access key'), 'AKIA')
    await user.type(screen.getByLabelText('Secret key'), 'secret')
    await waitFor(() => expect(submit).toBeEnabled())
  })

  it('posts the new storage to /s3-storages', async () => {
    const fetchMock = mockFetch({ ...baseRoutes, 'GET /s3-storages': [] })
    const user = userEvent.setup()
    renderApp(<Settings />)

    await screen.findByRole('button', { name: /add s3 storage/i })
    await user.type(screen.getByPlaceholderText('backups'), 'my-s3')
    await user.type(screen.getByPlaceholderText('my-bucket'), 'bkt')
    await user.type(screen.getByLabelText('Access key'), 'AKIA')
    await user.type(screen.getByLabelText('Secret key'), 'sk')
    await user.click(screen.getByRole('button', { name: /add s3 storage/i }))

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/s3-storages' && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
      const body = JSON.parse(String(call?.[1]?.body))
      expect(body.name).toBe('my-s3')
      expect(body.bucket).toBe('bkt')
      expect(body.key).toBe('AKIA')
      expect(body.secret).toBe('sk')
    })
  })
})
