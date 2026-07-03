import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { Outlet, Route, Routes } from 'react-router'
import ApplicationPreviews from './previews'
import type { ApplicationContext } from './index'
import { mockFetch, renderApp } from '../../../test/harness'

const app = { uuid: 'app1', name: 'web', status: 'running' } as unknown as ApplicationContext['app']

function ContextProvider() {
  return <Outlet context={{ app, refetch: () => {} } satisfies ApplicationContext} />
}

function renderTab() {
  return renderApp(
    <Routes>
      <Route element={<ContextProvider />}>
        <Route path="*" element={<ApplicationPreviews />} />
      </Route>
    </Routes>,
  )
}

const previews = [
  {
    uuid: 'pv1',
    pull_request_id: 7,
    pull_request_html_url: 'https://github.com/acme/web/pull/7',
    fqdn: 'https://pr-7.example.com',
    status: 'running',
    git_type: 'github',
    last_online_at: null,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  },
]

describe('preview deployments tab', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('renders previews from the mocked list', async () => {
    mockFetch({ 'GET /applications/app1/previews': previews })
    renderTab()

    expect(await screen.findByText('PR #7')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'https://pr-7.example.com' })).toHaveAttribute(
      'href',
      'https://pr-7.example.com',
    )
  })

  it('redeploy posts and cleanup deletes for the PR', async () => {
    const fetchMock = mockFetch({
      'GET /applications/app1/previews': previews,
      'POST /applications/app1/previews/7/redeploy': { deployment_uuid: 'd1' },
      'DELETE /applications/app1/previews/7': null,
    })
    const user = userEvent.setup()
    renderTab()

    await screen.findByText('PR #7')
    await user.click(screen.getByRole('button', { name: /redeploy/i }))
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.find(
          ([url, init]) =>
            url === '/api/v1/applications/app1/previews/7/redeploy' && init?.method === 'POST',
        ),
      ).toBeTruthy(),
    )

    await user.click(screen.getByRole('button', { name: /cleanup/i }))
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.find(
          ([url, init]) =>
            url === '/api/v1/applications/app1/previews/7' && init?.method === 'DELETE',
        ),
      ).toBeTruthy(),
    )
  })
})
