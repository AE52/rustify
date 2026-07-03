import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import TeamsPage from './teams'
import { mockFetch, renderApp } from '../test/harness'

const adminTeam = {
  id: 5,
  uuid: 't5',
  name: 'Alpha',
  description: 'the team',
  personal_team: false,
  role: 'admin',
}
const memberTeam = { ...adminTeam, role: 'member' }
const members = [
  { uuid: 'u1', email: 'a@b.c', name: 'Alice', role: 'admin' },
  { uuid: 'u2', email: 'b@b.c', name: 'Bob', role: 'member' },
]

afterEach(() => vi.unstubAllGlobals())

describe('team settings', () => {
  it('changes a member role via PATCH', async () => {
    const fetchMock = mockFetch({
      'GET /teams/current': adminTeam,
      'GET /teams/current/members': members,
      'GET /teams/5/invitations': [],
      'PATCH /teams/5/members/u2': { ...members[1], role: 'admin' },
    })
    const user = userEvent.setup()
    renderApp(<TeamsPage />)

    const roleSelect = await screen.findByLabelText('Role for b@b.c')
    await user.selectOptions(roleSelect, 'admin')

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/teams/5/members/u2' && init?.method === 'PATCH',
      )
      expect(call).toBeTruthy()
      expect(JSON.parse(String(call?.[1]?.body)).role).toBe('admin')
    })
  })

  it('creates an invitation and reveals its link', async () => {
    const fetchMock = mockFetch({
      'GET /teams/current': adminTeam,
      'GET /teams/current/members': members,
      'GET /teams/5/invitations': [],
      'POST /teams/5/invitations': {
        uuid: 'inv1',
        email: 'new@b.c',
        role: 'member',
        via: 'link',
        link: '/invitations/inv1',
        created_at: '2026-07-03T00:00:00Z',
      },
    })
    const user = userEvent.setup()
    renderApp(<TeamsPage />)

    await screen.findByText('Invitations')
    await user.type(screen.getByPlaceholderText('teammate@example.com'), 'new@b.c')
    await user.click(screen.getByRole('button', { name: /invite/i }))

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/teams/5/invitations' && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
      expect(JSON.parse(String(call?.[1]?.body)).email).toBe('new@b.c')
    })
    expect(await screen.findByTestId('invite-link')).toHaveTextContent('/invitations/inv1')
  })

  it('hides management controls for a member', async () => {
    mockFetch({
      'GET /teams/current': memberTeam,
      'GET /teams/current/members': members,
    })
    renderApp(<TeamsPage />)

    // Member-view: read-only roles, no invitations section, no remove buttons.
    await screen.findByText('Members')
    expect(screen.queryByLabelText('Role for b@b.c')).not.toBeInTheDocument()
    expect(screen.queryByText('Invitations')).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /remove/i })).not.toBeInTheDocument()
  })
})
