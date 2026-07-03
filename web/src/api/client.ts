import type { components } from './types.gen'

export type Schemas = components['schemas']

// The server (utoipa) names response schemas after their Rust DTO types
// (`*Dto`) and inlines the `build_pack`/`status` enums, so the friendly aliases
// the app uses are mapped onto the generated names here.
export type User = Schemas['UserDto']
export type PrivateKey = Schemas['PrivateKeyDto']
export type Server = Schemas['ServerDto']
export type ProxyConfig = Schemas['ProxyConfig']
export type Project = Schemas['ProjectDto']
export type Environment = Schemas['EnvironmentDto']
export type Application = Schemas['ApplicationDto']
export type ApplicationCreate = Schemas['ApplicationCreate']
export type BuildPack =
  | 'nixpacks'
  | 'dockerfile'
  | 'static'
  | 'docker_image'
  | 'docker_compose'
  | 'railpack'
export type EnvVar = Schemas['EnvVarDto']
export type Deployment = Schemas['DeploymentDto']
export type DeploymentDetail = Schemas['DeploymentDetailDto']
export type DeploymentStatus = 'queued' | 'in_progress' | 'finished' | 'failed' | 'cancelled'
export type LogLine = Schemas['LogLineDto']
export type InstanceSettings = Schemas['InstanceSettingsDto']
export type ApiToken = Schemas['ApiTokenDto']
export type ApiTokenCreated = Schemas['ApiTokenCreated']

// ----- Phase 2 resources -------------------------------------------------
export type Database = Schemas['DatabaseDto']
export type DatabaseCreate = Schemas['DatabaseCreate']
export type DatabaseUpdate = Schemas['DatabaseUpdate']

export type Service = Schemas['ServiceDto']
export type ServiceApplication = Schemas['ServiceApplicationDto']
export type ServiceCreate = Schemas['ServiceCreate']
export type ServiceTemplate = Schemas['ServiceTemplateDto']
export type ServiceTemplateDetail = Schemas['ServiceTemplateDetailDto']

export type Backup = Schemas['BackupDto']
export type BackupCreate = Schemas['BackupCreate']
export type BackupExecution = Schemas['ExecutionDto']

export type S3Storage = Schemas['S3StorageDto']
export type S3StorageCreate = Schemas['S3StorageCreate']
export type S3TestResponse = Schemas['S3TestResponse']

export type ScheduledTask = Schemas['ScheduledTaskDto']
export type ScheduledTaskCreate = Schemas['ScheduledTaskCreate']
export type ScheduledTaskExecution = Schemas['ScheduledTaskExecutionDto']

// ----- Phase 3 resources -------------------------------------------------
export type GithubApp = Schemas['GithubAppDto']
export type GithubAppCreate = Schemas['GithubAppCreate']
export type GithubAppUpdate = Schemas['GithubAppUpdate']
export type RepositoriesResponse = Schemas['RepositoriesResponse']
export type BranchesResponse = Schemas['BranchesResponse']
export type ManifestStateResponse = Schemas['ManifestStateResponse']

export type NotificationSettings = Schemas['NotificationSettingsDto']
export type NotificationSettingsUpdate = Schemas['NotificationSettingsUpdate']
export type NotificationTestRequest = Schemas['TestRequest']
export type NotificationTestResponse = Schemas['TestResponse']

export type Preview = Schemas['PreviewDto']
export type PreviewRedeployResponse = Schemas['PreviewRedeployResponse']

// ----- Phase 4 resources -------------------------------------------------
export type Team = Schemas['TeamDto']
export type TeamMember = Schemas['MemberDto']
export type TeamCreate = Schemas['TeamCreate']
export type TeamUpdate = Schemas['TeamUpdate']
export type TeamInvitation = Schemas['InvitationDto']
export type InvitationInfo = Schemas['InvitationInfo']
export type InvitationCreate = Schemas['InvitationCreate']
export type RoleUpdate = Schemas['RoleUpdate']
/** Team roles (§5); mirrors the server's `Role` string. */
export type Role = 'owner' | 'admin' | 'member'

export type CloudToken = Schemas['CloudTokenDto']
export type CloudTokenCreate = Schemas['CloudTokenCreate']
export type HetznerProvision = Schemas['HetznerProvision']
export type HetznerProvisionResponse = Schemas['HetznerProvisionResponse']

export type ServerSettings = Schemas['ServerSettingsDto']
export type ServerSettingsUpdate = Schemas['ServerSettingsUpdate']

/** A single metrics sample: `[unix_time_seconds, value]` (contract C5). */
export type MetricPoint = [number, number]

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
