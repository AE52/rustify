import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import NewServerPage from './new'
import { renderApp } from '../../test/harness'

/** Query-string-agnostic fetch: matches `METHOD pathname`. */
function stub(routes: Record<string, unknown>) {
  const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
    const method = (init?.method ?? 'GET').toUpperCase()
    const pathname = String(url).replace('/api/v1', '').split('?')[0]
    const body = routes[`${method} ${pathname}`] ?? []
    return new Response(JSON.stringify(body), {
      status: method === 'POST' ? 201 : 200,
      headers: { 'content-type': 'application/json' },
    })
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

afterEach(() => vi.unstubAllGlobals())

describe('Hetzner provisioning', () => {
  it('loads lookups after picking a token and submits a provision request', async () => {
    const fetchMock = stub({
      'GET /cloud-tokens': [
        { uuid: 'tok1', provider: 'hetzner', name: 'my token', created_at: '2026-01-01T00:00:00Z' },
      ],
      'GET /private-keys': [{ uuid: 'k1', name: 'key', team_id: 1 }],
      'GET /hetzner/locations': [{ id: 1, name: 'nbg1', city: 'Nuremberg' }],
      'GET /hetzner/server-types': [{ id: 22, name: 'cx11', cores: 1, memory: 2 }],
      'GET /hetzner/images': [{ id: 67794396, name: 'ubuntu-22.04', description: 'Ubuntu 22.04' }],
      'POST /servers/provision/hetzner': { uuid: 'srvnew', hetzner_server_id: 123, ip: '1.2.3.4' },
    })
    const user = userEvent.setup()
    renderApp(<NewServerPage />)

    // Pick the token -> triggers the Hetzner lookups.
    const tokenSelect = await screen.findByLabelText('Cloud token')
    await screen.findByRole('option', { name: /my token/ })
    await user.selectOptions(tokenSelect, 'tok1')

    // Options load from /hetzner/*.
    await screen.findByRole('option', { name: /cx11/ })
    await screen.findByRole('option', { name: /nbg1/ })
    await screen.findByRole('option', { name: /Ubuntu 22.04/ })

    await user.type(screen.getByLabelText('Name'), 'edge-1')
    await user.selectOptions(screen.getByLabelText('Server type'), 'cx11')
    await user.selectOptions(screen.getByLabelText('Location'), 'nbg1')
    await user.selectOptions(screen.getByLabelText('Image'), '67794396')

    await user.click(screen.getByRole('button', { name: /provision server/i }))

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) =>
          String(url).includes('/servers/provision/hetzner') && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
      const body = JSON.parse(String(call?.[1]?.body))
      expect(body).toMatchObject({
        token_uuid: 'tok1',
        name: 'edge-1',
        server_type: 'cx11',
        location: 'nbg1',
        image: 67794396,
      })
    })
  })
})
