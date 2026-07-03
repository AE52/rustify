import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { NewAppForm } from './[uuid]'
import { mockFetch, renderApp } from '../../test/harness'

const server = { uuid: 's1', name: 'edge' }

function baseRoutes(overrides: Record<string, unknown> = {}) {
  return {
    'GET /servers': [server],
    'GET /private-keys': [{ uuid: 'k1', name: 'deploy-key', public_key: 'ssh-rsa x', created_at: '', updated_at: '' }],
    'GET /github-apps': [{ uuid: 'gh1', name: 'prod', html_url: 'https://github.com', api_url: '', custom_user: 'git', custom_port: 22, is_public: false, is_system_wide: false, installation_id: 42, created_at: '', updated_at: '' }],
    'GET /github-apps/gh1/repositories': {
      repositories: [
        { id: 1, name: 'web', full_name: 'acme/web', owner: { login: 'acme' }, private: true, default_branch: 'main' },
      ],
    },
    'GET /github-apps/gh1/repositories/acme/web/branches': { branches: [{ name: 'main' }] },
    'POST /applications': (b: unknown) => ({ uuid: 'app1', ...(b as object) }),
    ...overrides,
  }
}

function lastPost(fetchMock: ReturnType<typeof mockFetch>) {
  const call = fetchMock.mock.calls
    .filter(([url, init]) => url === '/api/v1/applications' && init?.method === 'POST')
    .pop()
  return call ? JSON.parse(String(call[1]?.body)) : null
}

describe('new application source picker', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('offers railpack in the build-pack selector', async () => {
    mockFetch(baseRoutes())
    renderApp(<NewAppForm projectUuid="p1" environmentName="production" onCreated={() => {}} />)
    await screen.findByLabelText('Source')
    expect(screen.getByRole('option', { name: 'railpack' })).toBeInTheDocument()
  })

  it('public source posts a plain git_repository payload', async () => {
    const fetchMock = mockFetch(baseRoutes())
    const user = userEvent.setup()
    renderApp(<NewAppForm projectUuid="p1" environmentName="production" onCreated={() => {}} />)

    await user.type(await screen.findByLabelText('Name'), 'site')
    await user.type(screen.getByPlaceholderText(/github.com\/acme\/app.git/i), 'https://github.com/acme/site.git')
    await user.click(screen.getByRole('button', { name: /create application/i }))

    await waitFor(() => {
      const body = lastPost(fetchMock)
      expect(body).toBeTruthy()
      expect(body.git_repository).toBe('https://github.com/acme/site.git')
      expect(body.source).toBeUndefined()
      expect(body.private_key_uuid).toBeUndefined()
    })
  })

  it('deploy-key source posts private_key_uuid + ssh url', async () => {
    const fetchMock = mockFetch(baseRoutes())
    const user = userEvent.setup()
    renderApp(<NewAppForm projectUuid="p1" environmentName="production" onCreated={() => {}} />)

    await user.type(await screen.findByLabelText('Name'), 'svc')
    await user.selectOptions(screen.getByLabelText('Source'), 'deploy_key')
    await user.selectOptions(await screen.findByLabelText('Private key'), 'k1')
    await user.type(screen.getByPlaceholderText(/git@github.com/i), 'git@github.com:acme/svc.git')
    await user.click(screen.getByRole('button', { name: /create application/i }))

    await waitFor(() => {
      const body = lastPost(fetchMock)
      expect(body).toBeTruthy()
      expect(body.private_key_uuid).toBe('k1')
      expect(body.git_repository).toBe('git@github.com:acme/svc.git')
      expect(body.source).toBeUndefined()
    })
  })

  it('github-app source posts source=github_app with the picked repo/branch', async () => {
    const fetchMock = mockFetch(baseRoutes())
    const user = userEvent.setup()
    renderApp(<NewAppForm projectUuid="p1" environmentName="production" onCreated={() => {}} />)

    await user.type(await screen.findByLabelText('Name'), 'gha')
    await user.selectOptions(screen.getByLabelText('Source'), 'github_app')
    await user.selectOptions(await screen.findByLabelText('GitHub App'), 'gh1')
    await user.selectOptions(await screen.findByLabelText('Repository'), 'acme/web')
    await screen.findByLabelText('Branch')
    await user.click(screen.getByRole('button', { name: /create application/i }))

    await waitFor(() => {
      const body = lastPost(fetchMock)
      expect(body).toBeTruthy()
      expect(body.source).toBe('github_app')
      expect(body.github_app_uuid).toBe('gh1')
      expect(body.git_repository).toBe('acme/web')
      expect(body.git_branch).toBe('main')
      expect(body.is_private).toBe(true)
    })
  })
})
