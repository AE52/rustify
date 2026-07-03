import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import SourcesPage from './index'
import { RepoBranchPicker } from './[uuid]'
import { mockFetch, renderApp } from '../../test/harness'

describe('GitHub App manual registration form', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('requires a name before submit and posts to /github-apps', async () => {
    const fetchMock = mockFetch({
      'GET /github-apps': [],
      'GET /private-keys': [],
      'POST /github-apps': (b: unknown) => ({ uuid: 'gh1', ...(b as object) }),
    })
    const user = userEvent.setup()
    renderApp(<SourcesPage />)

    await user.click(await screen.findByRole('button', { name: /register manually/i }))

    const submit = screen.getByRole('button', { name: /register github app/i })
    expect(submit).toBeDisabled()

    await user.type(screen.getByPlaceholderText('my-github-app'), 'prod-app')
    await waitFor(() => expect(submit).toBeEnabled())

    await user.type(screen.getByLabelText('App ID'), '123')
    await user.click(submit)

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/github-apps' && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
      const body = JSON.parse(String(call?.[1]?.body))
      expect(body.name).toBe('prod-app')
      expect(body.app_id).toBe(123)
      expect(body.api_url).toBe('https://api.github.com')
    })
  })
})

describe('repo/branch picker', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('loads repositories then branches from mocked responses', async () => {
    mockFetch({
      'GET /github-apps/gh1/repositories': {
        repositories: [
          {
            id: 1,
            name: 'web',
            full_name: 'acme/web',
            owner: { login: 'acme' },
            private: true,
            default_branch: 'main',
          },
        ],
      },
      'GET /github-apps/gh1/repositories/acme/web/branches': {
        branches: [{ name: 'main' }, { name: 'feature/x' }],
      },
    })
    const user = userEvent.setup()
    renderApp(<RepoBranchPicker appUuid="gh1" />)

    const repoSelect = await screen.findByLabelText('Repository')
    await waitFor(() => expect(screen.getByRole('option', { name: /acme\/web/i })).toBeInTheDocument())

    await user.selectOptions(repoSelect, 'acme/web')

    const branchSelect = await screen.findByLabelText('Branch')
    await waitFor(() =>
      expect(screen.getByRole('option', { name: 'feature/x' })).toBeInTheDocument(),
    )
    expect(branchSelect).toBeInTheDocument()
  })
})
