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

describe('AWS provisioning', () => {
  const awsRoutes = {
    'GET /cloud-tokens': [
      { uuid: 'awstok', provider: 'aws', name: 'aws key', created_at: '2026-01-01T00:00:00Z' },
    ],
    'GET /private-keys': [{ uuid: 'k1', name: 'key', team_id: 1 }],
    'GET /aws/regions': [{ name: 'eu-central-1' }, { name: 'us-east-1' }],
    'GET /aws/instance-types': [
      { name: 't3.small', vcpus: 2, mem_gb: 2 },
      { name: 'm5.large', vcpus: 2, mem_gb: 8 },
    ],
  }

  it('adds an AWS cloud token with masked credentials', async () => {
    const fetchMock = stub({ 'GET /cloud-tokens': [], 'GET /private-keys': [] })
    const user = userEvent.setup()
    renderApp(<NewServerPage />)

    await user.selectOptions(await screen.findByLabelText('Token provider'), 'aws')
    const accessKey = screen.getByLabelText('Access key ID')
    const secretKey = screen.getByLabelText('Secret access key')
    expect(secretKey).toHaveAttribute('type', 'password')
    await user.type(accessKey, 'AKIAEXAMPLE')
    await user.type(secretKey, 'shhh')
    await user.click(screen.getByRole('button', { name: /add token/i }))

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => String(url).includes('/cloud-tokens') && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
      expect(JSON.parse(String(call?.[1]?.body))).toMatchObject({
        provider: 'aws',
        access_key_id: 'AKIAEXAMPLE',
        secret_access_key: 'shhh',
      })
    })
  })

  it('loads AWS options after picking a token and submits a provision request', async () => {
    const fetchMock = stub({
      ...awsRoutes,
      'POST /servers/provision/aws': {
        servers: [{ uuid: 'awssrv', name: 'edge-1', ip: '3.3.3.3' }],
        swarm: false,
        partial: false,
      },
    })
    const user = userEvent.setup()
    renderApp(<NewServerPage />)

    await user.click(screen.getByRole('button', { name: /provision on aws/i }))

    const tokenSelect = await screen.findByLabelText('AWS cloud token')
    await screen.findByRole('option', { name: /aws key/ })
    await user.selectOptions(tokenSelect, 'awstok')

    await screen.findByRole('option', { name: /eu-central-1/ })
    await screen.findByRole('option', { name: /t3.small — 2 vCPU \/ 2GB/ })

    await user.type(screen.getByLabelText('Name'), 'edge-1')
    await user.selectOptions(screen.getByLabelText('Region'), 'eu-central-1')
    await user.selectOptions(screen.getByLabelText('Instance type'), 't3.small')

    await user.click(screen.getByRole('button', { name: /provision server/i }))

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => String(url).includes('/servers/provision/aws') && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
      expect(JSON.parse(String(call?.[1]?.body))).toMatchObject({
        token_uuid: 'awstok',
        region: 'eu-central-1',
        instance_type: 't3.small',
        count: 1,
        name: 'edge-1',
      })
    })
  })

  it('labels multi-node as a swarm cluster and lists the created servers', async () => {
    stub({
      ...awsRoutes,
      'POST /servers/provision/aws': {
        servers: [
          { uuid: 's1', name: 'edge-1', ip: '1.1.1.1' },
          { uuid: 's2', name: 'edge-2', ip: '2.2.2.2' },
          { uuid: 's3', name: 'edge-3', ip: '3.3.3.3' },
        ],
        swarm: true,
        partial: false,
      },
    })
    const user = userEvent.setup()
    renderApp(<NewServerPage />)

    await user.click(screen.getByRole('button', { name: /provision on aws/i }))
    await user.selectOptions(await screen.findByLabelText('AWS cloud token'), 'awstok')
    await user.type(screen.getByLabelText('Name'), 'edge')
    await user.selectOptions(await screen.findByLabelText('Region'), 'eu-central-1')
    await user.selectOptions(screen.getByLabelText('Instance type'), 't3.small')

    const nodes = screen.getByLabelText('Nodes')
    await user.clear(nodes)
    await user.type(nodes, '3')
    expect(screen.getByText(/Docker Swarm cluster of 3/)).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: /provision server/i }))

    // Multi-node: no redirect — the created servers are listed with links.
    expect(await screen.findByRole('link', { name: /edge-2/ })).toHaveAttribute(
      'href',
      '/servers/s2',
    )
    expect(screen.getByText(/Docker Swarm cluster/)).toBeInTheDocument()
  })
})

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
