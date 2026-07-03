import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { Route, Routes } from 'react-router'
import InvitationPage from './[uuid]'
import { mockFetch, renderApp } from '../../test/harness'

afterEach(() => vi.unstubAllGlobals())

function renderInvite() {
  return renderApp(
    <Routes>
      <Route path="/invitations/:uuid" element={<InvitationPage />} />
    </Routes>,
    '/invitations/inv9',
  )
}

describe('invitation accept page', () => {
  it('renders the invitation and accepts it', async () => {
    const fetchMock = mockFetch({
      'GET /invitations/inv9': {
        uuid: 'inv9',
        email: 'new@b.c',
        role: 'admin',
        team_name: 'Alpha',
        valid: true,
        already_member: false,
      },
      'POST /invitations/inv9': { id: 5, uuid: 't5', name: 'Alpha', personal_team: false, role: 'admin' },
    })
    const user = userEvent.setup()
    renderInvite()

    await screen.findByText('Alpha')
    await user.click(screen.getByRole('button', { name: /accept invitation/i }))

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/invitations/inv9' && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
    })
  })

  it('shows an expired invitation without an accept button', async () => {
    mockFetch({
      'GET /invitations/inv9': {
        uuid: 'inv9',
        email: 'new@b.c',
        role: 'member',
        team_name: 'Alpha',
        valid: false,
        already_member: false,
      },
    })
    renderInvite()

    await screen.findByText(/expired/i)
    expect(screen.queryByRole('button', { name: /accept invitation/i })).not.toBeInTheDocument()
  })
})
