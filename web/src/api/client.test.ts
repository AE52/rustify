import { afterEach, describe, expect, it, vi } from 'vitest'
import { api, ApiError } from './client'

const jsonResponse = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  })

describe('api client', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('attaches credentials include and api prefix', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse([]))
    vi.stubGlobal('fetch', fetchMock)

    await api.get('/servers')

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/v1/servers')
    expect(init.credentials).toBe('include')
  })

  it('sends json bodies with content-type', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(jsonResponse({ user: { id: 'u1', email: 'a@b.c', name: 'A' } }))
    vi.stubGlobal('fetch', fetchMock)

    await api.post('/auth/login', { email: 'a@b.c', password: 'pw' })

    const [, init] = fetchMock.mock.calls[0]
    expect(init.method).toBe('POST')
    expect(init.credentials).toBe('include')
    expect(new Headers(init.headers).get('content-type')).toBe('application/json')
    expect(JSON.parse(init.body)).toEqual({ email: 'a@b.c', password: 'pw' })
  })

  it('throws ApiError with code and message from error envelope', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse({ code: 'unauthorized', message: 'no session' }, 401)),
    )

    const err: unknown = await api.get('/auth/me').catch((e: unknown) => e)

    expect(err).toBeInstanceOf(ApiError)
    const apiErr = err as ApiError
    expect(apiErr.code).toBe('unauthorized')
    expect(apiErr.message).toBe('no session')
    expect(apiErr.status).toBe(401)
  })

  it('throws ApiError even when error body is not json', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(new Response('boom', { status: 500 })),
    )

    const err: unknown = await api.get('/servers').catch((e: unknown) => e)

    expect(err).toBeInstanceOf(ApiError)
    expect((err as ApiError).status).toBe(500)
  })

  it('returns undefined for 204 responses', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 204 })))

    await expect(api.post('/auth/logout')).resolves.toBeUndefined()
  })
})
