import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import NewDatabase from './new'
import { DATABASE_ENGINES } from '../../lib/engines'
import { mockFetch, renderApp } from '../../test/harness'

const project = { uuid: 'p1', name: 'Proj', team_uuid: 't1' }
const server = { uuid: 's1', name: 'Srv' }
const env = { uuid: 'e1', name: 'production', project_uuid: 'p1' }

function setup() {
  return mockFetch({
    'GET /projects': [project],
    'GET /servers': [server],
    'GET /projects/p1/environments': [env],
  })
}

describe('NewDatabase form', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('lists all eight database engines', async () => {
    setup()
    renderApp(<NewDatabase />)
    for (const eng of DATABASE_ENGINES) {
      expect(screen.getByRole('option', { name: eng.label })).toBeInTheDocument()
    }
    expect(DATABASE_ENGINES).toHaveLength(8)
  })

  it('disables submit until a name is entered', async () => {
    setup()
    const user = userEvent.setup()
    renderApp(<NewDatabase />)

    // wait for project/server queries to populate the defaults
    await waitFor(() => expect(screen.getByRole('option', { name: 'Proj' })).toBeInTheDocument())

    const submit = screen.getByRole('button', { name: /create database/i })
    expect(submit).toBeDisabled()

    await user.type(screen.getByPlaceholderText('my-database'), 'orders-db')
    await waitFor(() => expect(submit).toBeEnabled())
  })

  it('posts the selected engine and name to /databases', async () => {
    const fetchMock = setup()
    const user = userEvent.setup()
    renderApp(<NewDatabase />)

    await waitFor(() => expect(screen.getByRole('option', { name: 'Proj' })).toBeInTheDocument())
    await user.selectOptions(screen.getByDisplayValue('PostgreSQL'), 'mysql')
    await user.type(screen.getByPlaceholderText('my-database'), 'orders-db')
    await user.click(screen.getByRole('button', { name: /create database/i }))

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/databases' && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
      const body = JSON.parse(String(call?.[1]?.body))
      expect(body.engine).toBe('mysql')
      expect(body.name).toBe('orders-db')
      expect(body.project_uuid).toBe('p1')
      expect(body.server_uuid).toBe('s1')
    })
  })
})
