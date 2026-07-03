import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { ServerSettingsTab } from './ServerSettingsTab'
import { mockFetch, renderApp } from '../test/harness'

const settings = {
  proxy_type: 'traefik',
  is_build_server: false,
  is_terminal_enabled: true,
  metrics_enabled: true,
  metrics_refresh_rate_seconds: 10,
  is_cloudflare_tunnel: false,
}
const adminTeam = { id: 1, uuid: 't1', name: 'Alpha', personal_team: true, role: 'admin' }

afterEach(() => vi.unstubAllGlobals())

describe('server ops settings', () => {
  it('PATCHes when the build-server toggle flips', async () => {
    const fetchMock = mockFetch({
      'GET /servers/s1/settings': settings,
      'GET /teams/current': adminTeam,
      'PATCH /servers/s1/settings': { ...settings, is_build_server: true },
    })
    const user = userEvent.setup()
    renderApp(<ServerSettingsTab serverUuid="s1" />)

    const toggle = await screen.findByRole('switch', { name: 'Build server' })
    await user.click(toggle)

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/servers/s1/settings' && init?.method === 'PATCH',
      )
      expect(call).toBeTruthy()
      expect(JSON.parse(String(call?.[1]?.body))).toEqual({ is_build_server: true })
    })
  })

  it('PATCHes the proxy type selection', async () => {
    const fetchMock = mockFetch({
      'GET /servers/s1/settings': settings,
      'GET /teams/current': adminTeam,
      'PATCH /servers/s1/settings': { ...settings, proxy_type: 'caddy' },
    })
    const user = userEvent.setup()
    renderApp(<ServerSettingsTab serverUuid="s1" />)

    const proxy = await screen.findByLabelText('Proxy type')
    await user.selectOptions(proxy, 'caddy')

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/servers/s1/settings' && init?.method === 'PATCH',
      )
      expect(JSON.parse(String(call?.[1]?.body))).toEqual({ proxy_type: 'caddy' })
    })
  })

  it('hides edit controls for a member', async () => {
    mockFetch({
      'GET /servers/s1/settings': settings,
      'GET /teams/current': { ...adminTeam, role: 'member' },
    })
    renderApp(<ServerSettingsTab serverUuid="s1" />)

    const toggle = await screen.findByRole('switch', { name: 'Build server' })
    expect(toggle).toBeDisabled()
  })
})
