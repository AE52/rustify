import type { components } from './types.gen'

export type Schemas = components['schemas']

export type User = Schemas['User']
export type PrivateKey = Schemas['PrivateKey']
export type Server = Schemas['Server']
export type ProxyConfig = Schemas['ProxyConfig']
export type Project = Schemas['Project']
export type Environment = Schemas['Environment']
export type Application = Schemas['Application']
export type ApplicationCreate = Schemas['ApplicationCreate']
export type BuildPack = Schemas['BuildPack']
export type EnvVar = Schemas['EnvVar']
export type Deployment = Schemas['Deployment']
export type DeploymentDetail = Schemas['DeploymentDetail']
export type DeploymentStatus = Schemas['DeploymentStatus']
export type LogLine = Schemas['LogLine']
export type InstanceSettings = Schemas['InstanceSettings']
export type ApiToken = Schemas['ApiToken']
export type ApiTokenCreated = Schemas['ApiTokenCreated']

const BASE = '/api/v1'

/** Error envelope per C5: `{"code": "...", "message": "..."}`. */
export class ApiError extends Error {
  code: string
  status: number

  constructor(status: number, code: string, message: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const init: RequestInit = {
    method,
    credentials: 'include',
  }
  if (body !== undefined) {
    init.headers = { 'content-type': 'application/json' }
    init.body = JSON.stringify(body)
  }

  const res = await fetch(`${BASE}${path}`, init)

  if (!res.ok) {
    let code = 'unknown'
    let message = res.statusText || `request failed with status ${res.status}`
    try {
      const envelope: unknown = await res.json()
      if (envelope && typeof envelope === 'object') {
        const e = envelope as { code?: unknown; message?: unknown }
        if (typeof e.code === 'string') code = e.code
        if (typeof e.message === 'string') message = e.message
      }
    } catch {
      // non-json error body; keep defaults
    }
    throw new ApiError(res.status, code, message)
  }

  if (res.status === 204) {
    return undefined as T
  }
  const text = await res.text()
  if (text.length === 0) {
    return undefined as T
  }
  return JSON.parse(text) as T
}

export const api = {
  get<T>(path: string): Promise<T> {
    return request<T>('GET', path)
  },
  post<T = void>(path: string, body?: unknown): Promise<T> {
    return request<T>('POST', path, body)
  },
  patch<T>(path: string, body: unknown): Promise<T> {
    return request<T>('PATCH', path, body)
  },
  delete(path: string): Promise<void> {
    return request<void>('DELETE', path)
  },
}
