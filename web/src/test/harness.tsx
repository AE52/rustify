import type { ReactElement } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router'
import { render } from '@testing-library/react'
import { vi } from 'vitest'

/** A route handler keyed by `METHOD /path` (path is without the /api/v1 prefix). */
export type Routes = Record<string, unknown | ((body: unknown) => unknown)>

/**
 * Install a `fetch` mock that resolves the api client's requests from a
 * `{ "GET /servers": [...] }` table. Returns the underlying vi.fn so callers can
 * assert on calls. Unmatched routes resolve to `[]` with status 200.
 */
export function mockFetch(routes: Routes) {
  const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
    const method = (init?.method ?? 'GET').toUpperCase()
    const path = String(url).replace('/api/v1', '')
    const key = `${method} ${path}`
    if (key in routes) {
      const entry = routes[key]
      const body =
        typeof entry === 'function'
          ? (entry as (b: unknown) => unknown)(init?.body ? JSON.parse(String(init.body)) : undefined)
          : entry
      return new Response(JSON.stringify(body ?? null), {
        status: method === 'POST' ? 201 : 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify([]), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

export function renderApp(ui: ReactElement, initialPath = '/') {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[initialPath]}>{ui}</MemoryRouter>
    </QueryClientProvider>,
  )
}
