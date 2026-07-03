import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { TeamSwitcher } from './TeamSwitcher'
import { mockFetch, renderApp } from '../test/harness'

const teams = [
  { id: 1, uuid: 't1', name: 'Alpha', personal_team: true, role: 'owner' },
  { id: 2, uuid: 't2', name: 'Beta', personal_team: false, role: 'admin' },
]

afterEach(() => vi.unstubAllGlobals())

describe('<TeamSwitcher />', () => {
  it('shows the active team and switches on selection', async () => {
    const fetchMock = mockFetch({
      'GET /teams/current': teams[0],
      'GET /teams': teams,
      'POST /teams/2/switch': teams[1],
    })
    const user = userEvent.setup()
    renderApp(<TeamSwitcher />)

    // Active team surfaced from /teams/current.
    await screen.findByText('Alpha')

    // Open the menu -> lists the user's teams.
    await user.click(screen.getByRole('button', { name: /switch team/i }))
    await screen.findByRole('menuitem', { name: /Beta/ })

    // Selecting Beta posts the switch.
    await user.click(screen.getByRole('menuitem', { name: /Beta/ }))
    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/teams/2/switch' && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
    })
  })
})
