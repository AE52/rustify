import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { Route, Routes } from 'react-router'
import DatabasePage from './[uuid]'
import { mockFetch, renderApp } from '../../test/harness'

const database = {
  uuid: 'db1',
  name: 'orders',
  description: null,
  engine: 'postgresql',
  image: 'postgres:16-alpine',
  status: 'running:healthy',
  environment_uuid: 'e1',
  project_uuid: 'p1',
  server_uuid: 's1',
  is_public: false,
  public_port: null,
  public_port_timeout: 30,
  ports_mappings: null,
  limits_memory: '0',
  limits_cpus: '0',
  health_check_enabled: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
}

const backup = {
  uuid: 'b1',
  database_uuid: 'db1',
  enabled: true,
  frequency: 'daily',
  save_s3: false,
  s3_storage_uuid: null,
  databases_to_backup: null,
  dump_all: false,
  disable_local_backup: false,
  retention_amount_local: 7,
  retention_days_local: 30,
  retention_max_gb_local: 0,
  retention_amount_s3: 0,
  retention_days_s3: 0,
  retention_max_gb_s3: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
}

function renderPage() {
  return renderApp(
    <Routes>
      <Route path="/databases/:uuid" element={<DatabasePage />} />
    </Routes>,
    '/databases/db1',
  )
}

describe('database backups tab', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('lists schedules and triggers a backup via the endpoint', async () => {
    const fetchMock = mockFetch({
      'GET /databases/db1': database,
      'GET /databases/db1/backups': [backup],
      'GET /s3-storages': [],
      'GET /backups/b1/executions': [],
      'POST /backups/b1/trigger': { status: 'accepted', execution_uuid: 'x1' },
    })
    const user = userEvent.setup()
    renderPage()

    await waitFor(() => expect(screen.getByText('orders')).toBeInTheDocument())

    await user.click(screen.getByRole('button', { name: 'backups' }))
    await waitFor(() => expect(screen.getByText('daily')).toBeInTheDocument())

    await user.click(screen.getByRole('button', { name: /trigger now/i }))

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/backups/b1/trigger' && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
    })
  })
})
