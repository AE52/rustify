// Client-side GitHub App manifest construction + form submission, ported from
// coolify resources/views/livewire/source/github/change.blade.php (manifest JS).

export interface ManifestOptions {
  name: string
  /** Instance base URL (origin), used for webhook/callback URLs. */
  baseUrl: string
  /** Request pull_request write + event (preview deployments). */
  previewDeployments?: boolean
  /** Request administration write. */
  administration?: boolean
}

export interface GithubManifest {
  name: string
  url: string
  hook_attributes: { url: string; active: boolean }
  redirect_url: string
  callback_urls: string[]
  public: boolean
  request_oauth_on_install: boolean
  setup_url: string
  setup_on_update: boolean
  default_permissions: Record<string, string>
  default_events: string[]
}

/** Build the GitHub App manifest payload (parity with Coolify's manifest data). */
export function buildManifest(opts: ManifestOptions): GithubManifest {
  const baseUrl = opts.baseUrl.replace(/\/+$/, '')
  const webhookBaseUrl = `${baseUrl}/webhooks`

  const default_permissions: Record<string, string> = {
    contents: 'read',
    metadata: 'read',
    emails: 'read',
    administration: 'read',
  }
  const default_events = ['push']
  if (opts.previewDeployments) {
    default_permissions.pull_requests = 'write'
    default_events.push('pull_request')
  }
  if (opts.administration) {
    default_permissions.administration = 'write'
  }

  return {
    name: opts.name,
    url: baseUrl,
    hook_attributes: { url: `${webhookBaseUrl}/source/github/events`, active: true },
    redirect_url: `${webhookBaseUrl}/source/github/redirect`,
    callback_urls: [`${baseUrl}/login/github/app`],
    public: false,
    request_oauth_on_install: false,
    setup_url: `${webhookBaseUrl}/source/github/install`,
    setup_on_update: true,
    default_permissions,
    default_events,
  }
}

/** Compute the GitHub `settings/apps/new` action URL, org-scoped when given. */
export function manifestAction(
  htmlUrl: string,
  state: string,
  organization?: string | null,
): string {
  const base = htmlUrl.replace(/\/+$/, '')
  const path = organization
    ? `organizations/${organization}/settings/apps/new`
    : 'settings/apps/new'
  return `${base}/${path}?state=${state}`
}

/**
 * Build a hidden form carrying the manifest JSON and POST it to GitHub. GitHub
 * then redirects back to the instance's `/webhooks/source/github/redirect`.
 * Returns the created form (already submitted) so callers/tests can inspect it.
 */
export function submitManifest(
  action: string,
  manifest: GithubManifest,
  doc: Document = document,
): HTMLFormElement {
  const form = doc.createElement('form')
  form.setAttribute('method', 'post')
  form.setAttribute('action', action)
  const input = doc.createElement('input')
  input.setAttribute('name', 'manifest')
  input.setAttribute('type', 'hidden')
  input.setAttribute('value', JSON.stringify(manifest))
  form.appendChild(input)
  doc.body.appendChild(form)
  form.submit()
  return form
}

// ----- repository shape helpers ------------------------------------------

export interface GithubRepo {
  id: number
  name: string
  full_name: string
  owner: string
  private: boolean
  default_branch: string
}

/** Normalize a raw GitHub `/installation/repositories` entry. */
export function parseRepo(raw: unknown): GithubRepo | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  const full_name = typeof r.full_name === 'string' ? r.full_name : ''
  if (!full_name) return null
  const ownerObj = r.owner as Record<string, unknown> | undefined
  const owner =
    typeof ownerObj?.login === 'string' ? ownerObj.login : full_name.split('/')[0]
  return {
    id: typeof r.id === 'number' ? r.id : 0,
    name: typeof r.name === 'string' ? r.name : full_name.split('/')[1] ?? full_name,
    full_name,
    owner,
    private: Boolean(r.private),
    default_branch: typeof r.default_branch === 'string' ? r.default_branch : 'main',
  }
}

/** Normalize a raw GitHub `/branches` entry to its name. */
export function parseBranch(raw: unknown): string | null {
  if (!raw || typeof raw !== 'object') return null
  const b = raw as Record<string, unknown>
  return typeof b.name === 'string' ? b.name : null
}
