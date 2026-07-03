import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router'
import {
  api,
  type GithubApp,
  type ManifestStateResponse,
  type PrivateKey,
} from '../../api/client'
import { buildManifest, manifestAction, submitManifest } from '../../lib/github'
import {
  btnGhost,
  btnPrimary,
  cardCls,
  ErrorNote,
  Field,
  inputCls,
  PageTitle,
  SectionTitle,
  selectCls,
} from '../../components/ui'

/** Manual registration: user pastes an existing App's credentials. */
function ManualForm({ onCreated }: { onCreated: () => void }) {
  const [name, setName] = useState('')
  const [apiUrl, setApiUrl] = useState('https://api.github.com')
  const [htmlUrl, setHtmlUrl] = useState('https://github.com')
  const [appId, setAppId] = useState('')
  const [installationId, setInstallationId] = useState('')
  const [clientId, setClientId] = useState('')
  const [clientSecret, setClientSecret] = useState('')
  const [webhookSecret, setWebhookSecret] = useState('')
  const [privateKeyUuid, setPrivateKeyUuid] = useState('')

  const keys = useQuery({ queryKey: ['private-keys'], queryFn: () => api.get<PrivateKey[]>('/private-keys') })

  const create = useMutation({
    mutationFn: () =>
      api.post<GithubApp>('/github-apps', {
        name,
        api_url: apiUrl,
        html_url: htmlUrl,
        app_id: appId.trim() ? Number(appId) : null,
        installation_id: installationId.trim() ? Number(installationId) : null,
        client_id: clientId.trim() || null,
        client_secret: clientSecret.trim() || null,
        webhook_secret: webhookSecret.trim() || null,
        private_key_uuid: privateKeyUuid || null,
      }),
    onSuccess: () => {
      setName('')
      setAppId('')
      setInstallationId('')
      setClientId('')
      setClientSecret('')
      setWebhookSecret('')
      setPrivateKeyUuid('')
      onCreated()
    },
  })

  const canSubmit = name.trim() !== '' && !create.isPending

  return (
    <form
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        if (canSubmit) create.mutate()
      }}
      className={`${cardCls} flex flex-col gap-3`}
    >
      <SectionTitle>Register an existing GitHub App</SectionTitle>
      <Field label="Name">
        <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} placeholder="my-github-app" />
      </Field>
      <div className="grid grid-cols-2 gap-3">
        <Field label="API URL">
          <input className={`${inputCls} font-mono`} value={apiUrl} onChange={(e) => setApiUrl(e.target.value)} />
        </Field>
        <Field label="HTML URL">
          <input className={`${inputCls} font-mono`} value={htmlUrl} onChange={(e) => setHtmlUrl(e.target.value)} />
        </Field>
      </div>
      <div className="grid grid-cols-2 gap-3">
        <Field label="App ID">
          <input
            className={`${inputCls} font-mono`}
            value={appId}
            onChange={(e) => setAppId(e.target.value)}
            inputMode="numeric"
          />
        </Field>
        <Field label="Installation ID">
          <input
            className={`${inputCls} font-mono`}
            value={installationId}
            onChange={(e) => setInstallationId(e.target.value)}
            inputMode="numeric"
          />
        </Field>
      </div>
      <Field label="Client ID">
        <input className={`${inputCls} font-mono`} value={clientId} onChange={(e) => setClientId(e.target.value)} />
      </Field>
      <div className="grid grid-cols-2 gap-3">
        <Field label="Client secret">
          <input
            type="password"
            className={`${inputCls} font-mono`}
            value={clientSecret}
            onChange={(e) => setClientSecret(e.target.value)}
            autoComplete="off"
          />
        </Field>
        <Field label="Webhook secret">
          <input
            type="password"
            className={`${inputCls} font-mono`}
            value={webhookSecret}
            onChange={(e) => setWebhookSecret(e.target.value)}
            autoComplete="off"
          />
        </Field>
      </div>
      <Field label="Private key (RSA PEM)">
        <select className={selectCls} value={privateKeyUuid} onChange={(e) => setPrivateKeyUuid(e.target.value)}>
          <option value="">— none —</option>
          {keys.data?.map((k) => (
            <option key={k.uuid} value={k.uuid}>
              {k.name}
            </option>
          ))}
        </select>
      </Field>
      <ErrorNote error={create.error} />
      <button type="submit" className={`${btnPrimary} w-fit`} disabled={!canSubmit}>
        {create.isPending ? 'Creating…' : 'Register GitHub App'}
      </button>
    </form>
  )
}

/** Manifest flow: create a stub App, then bounce to GitHub to mint credentials. */
function ManifestForm({ onCreated }: { onCreated: () => void }) {
  const [name, setName] = useState('')
  const [apiUrl, setApiUrl] = useState('https://api.github.com')
  const [htmlUrl, setHtmlUrl] = useState('https://github.com')
  const [organization, setOrganization] = useState('')
  const [previewDeployments, setPreviewDeployments] = useState(true)

  const create = useMutation({
    mutationFn: async () => {
      const app = await api.post<GithubApp>('/github-apps', {
        name,
        api_url: apiUrl,
        html_url: htmlUrl,
        organization: organization.trim() || null,
      })
      const { state } = await api.post<ManifestStateResponse>(`/github-apps/${app.uuid}/manifest-state`)
      const manifest = buildManifest({
        name,
        baseUrl: window.location.origin,
        previewDeployments,
      })
      const action = manifestAction(htmlUrl, state, organization.trim() || null)
      onCreated()
      submitManifest(action, manifest)
      return app
    },
  })

  const canSubmit = name.trim() !== '' && !create.isPending

  return (
    <form
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        if (canSubmit) create.mutate()
      }}
      className={`${cardCls} flex flex-col gap-3`}
    >
      <SectionTitle>Create a new GitHub App</SectionTitle>
      <p className="text-xs text-zinc-500">
        Registers an App via GitHub's manifest flow. Permissions and webhooks are pre-configured.
      </p>
      <Field label="Name">
        <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} placeholder="rustify" />
      </Field>
      <div className="grid grid-cols-2 gap-3">
        <Field label="API URL">
          <input className={`${inputCls} font-mono`} value={apiUrl} onChange={(e) => setApiUrl(e.target.value)} />
        </Field>
        <Field label="HTML URL">
          <input className={`${inputCls} font-mono`} value={htmlUrl} onChange={(e) => setHtmlUrl(e.target.value)} />
        </Field>
      </div>
      <Field label="Organization (blank = personal account)">
        <input
          className={inputCls}
          value={organization}
          onChange={(e) => setOrganization(e.target.value)}
          placeholder="acme"
        />
      </Field>
      <label className="flex items-center gap-2 text-sm text-zinc-300">
        <input
          type="checkbox"
          checked={previewDeployments}
          onChange={(e) => setPreviewDeployments(e.target.checked)}
          className="accent-zinc-400"
        />
        Request pull-request permissions (preview deployments)
      </label>
      <ErrorNote error={create.error} />
      <button type="submit" className={`${btnPrimary} w-fit`} disabled={!canSubmit}>
        {create.isPending ? 'Redirecting…' : 'Create GitHub App'}
      </button>
    </form>
  )
}

export default function SourcesPage() {
  const queryClient = useQueryClient()
  const [mode, setMode] = useState<'none' | 'manifest' | 'manual'>('none')

  const apps = useQuery({ queryKey: ['github-apps'], queryFn: () => api.get<GithubApp[]>('/github-apps') })

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ['github-apps'] })
    setMode('none')
  }

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center justify-between">
        <PageTitle>Sources</PageTitle>
        <div className="flex gap-2">
          <button type="button" className={btnGhost} onClick={() => setMode(mode === 'manifest' ? 'none' : 'manifest')}>
            {mode === 'manifest' ? 'Close' : 'Create GitHub App'}
          </button>
          <button type="button" className={btnGhost} onClick={() => setMode(mode === 'manual' ? 'none' : 'manual')}>
            {mode === 'manual' ? 'Close' : 'Register manually'}
          </button>
        </div>
      </div>

      {mode === 'manifest' && <ManifestForm onCreated={invalidate} />}
      {mode === 'manual' && <ManualForm onCreated={invalidate} />}

      <section className="flex flex-col gap-2">
        <SectionTitle>GitHub Apps</SectionTitle>
        <ErrorNote error={apps.error} />
        {apps.data?.map((g) => (
          <Link
            key={g.uuid}
            to={`/sources/github/${g.uuid}`}
            className="flex items-center justify-between gap-3 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-2.5 hover:border-zinc-600"
          >
            <div className="min-w-0">
              <span className="font-medium text-zinc-100">{g.name}</span>
              <span className="ml-3 truncate font-mono text-xs text-zinc-500">{g.html_url}</span>
            </div>
            <span
              className={`shrink-0 text-xs ${g.installation_id ? 'text-emerald-400' : 'text-amber-400'}`}
            >
              {g.installation_id ? 'installed' : 'not installed'}
            </span>
          </Link>
        ))}
        {apps.data?.length === 0 && <p className="text-sm text-zinc-500">No GitHub Apps yet.</p>}
      </section>
    </div>
  )
}
