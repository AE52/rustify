import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  api,
  type ApiToken,
  type ApiTokenCreated,
  type InstanceSettings,
  type S3Storage,
  type S3TestResponse,
} from '../api/client'
import {
  btnDanger,
  btnGhost,
  btnPrimary,
  cardCls,
  ErrorNote,
  Field,
  inputCls,
  PageTitle,
  SectionTitle,
} from '../components/ui'

function InstanceSection() {
  const queryClient = useQueryClient()
  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<InstanceSettings>('/settings'),
  })
  const [fqdn, setFqdn] = useState<string | null>(null)
  const [wildcard, setWildcard] = useState<string | null>(null)
  const [registration, setRegistration] = useState<boolean | null>(null)
  const [prPublic, setPrPublic] = useState<boolean | null>(null)

  const save = useMutation({
    mutationFn: () =>
      api.patch<InstanceSettings>('/settings', {
        fqdn: (fqdn ?? settings.data?.fqdn ?? '') || null,
        wildcard_domain: (wildcard ?? settings.data?.wildcard_domain ?? '') || null,
        registration_enabled: registration ?? settings.data?.registration_enabled ?? false,
        is_pr_deployments_public_enabled:
          prPublic ?? settings.data?.is_pr_deployments_public_enabled ?? false,
      }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['settings'] }),
  })

  if (settings.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (settings.isError) return <ErrorNote error={settings.error} />

  const s = settings.data

  return (
    <form
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        save.mutate()
      }}
      className={`${cardCls} flex max-w-2xl flex-col gap-4`}
    >
      <SectionTitle>Instance</SectionTitle>
      <Field label="Instance FQDN">
        <input
          className={`${inputCls} font-mono`}
          value={fqdn ?? s.fqdn ?? ''}
          onChange={(e) => setFqdn(e.target.value)}
          placeholder="https://rustify.example.com"
        />
      </Field>
      <Field label="Wildcard domain for new applications">
        <input
          className={`${inputCls} font-mono`}
          value={wildcard ?? s.wildcard_domain ?? ''}
          onChange={(e) => setWildcard(e.target.value)}
          placeholder="*.apps.example.com"
        />
      </Field>
      <label className="flex items-center gap-2 text-sm text-zinc-300">
        <input
          type="checkbox"
          checked={registration ?? s.registration_enabled}
          onChange={(e) => setRegistration(e.target.checked)}
          className="accent-zinc-400"
        />
        Allow new user registration
      </label>
      <label className="flex items-center gap-2 text-sm text-zinc-300">
        <input
          type="checkbox"
          checked={prPublic ?? s.is_pr_deployments_public_enabled}
          onChange={(e) => setPrPublic(e.target.checked)}
          className="accent-zinc-400"
          aria-label="Public PR deployments enabled"
        />
        Allow preview deployments from forks / public pull requests
      </label>
      <ErrorNote error={save.error} />
      <button type="submit" className={`${btnPrimary} w-fit`} disabled={save.isPending}>
        {save.isPending ? 'Saving…' : 'Save'}
      </button>
    </form>
  )
}

function TokensSection() {
  const queryClient = useQueryClient()
  const [name, setName] = useState('')
  const [created, setCreated] = useState<ApiTokenCreated | null>(null)
  const [copied, setCopied] = useState(false)

  const tokens = useQuery({
    queryKey: ['api-tokens'],
    queryFn: () => api.get<ApiToken[]>('/api-tokens'),
  })

  const create = useMutation({
    mutationFn: () => api.post<ApiTokenCreated>('/api-tokens', { name }),
    onSuccess: (token) => {
      setCreated(token)
      setCopied(false)
      setName('')
      queryClient.invalidateQueries({ queryKey: ['api-tokens'] })
    },
  })

  const remove = useMutation({
    mutationFn: (uuid: string) => api.delete(`/api-tokens/${uuid}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['api-tokens'] }),
  })

  const copy = async () => {
    if (!created) return
    try {
      await navigator.clipboard.writeText(created.token)
      setCopied(true)
    } catch {
      // clipboard unavailable; user can select manually
    }
  }

  return (
    <section className={`${cardCls} flex max-w-2xl flex-col gap-4`}>
      <SectionTitle>API tokens</SectionTitle>

      <form
        onSubmit={(e: FormEvent) => {
          e.preventDefault()
          if (name.trim()) create.mutate()
        }}
        className="flex items-end gap-2"
      >
        <div className="flex-1">
          <Field label="Token name">
            <input
              className={inputCls}
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="ci-deploy"
            />
          </Field>
        </div>
        <button type="submit" className={btnPrimary} disabled={!name.trim() || create.isPending}>
          {create.isPending ? 'Creating…' : 'Create token'}
        </button>
      </form>
      <ErrorNote error={create.error} />

      {created && (
        <div className="flex flex-col gap-2 rounded-lg border border-amber-600/40 bg-amber-950/20 p-3">
          <p className="text-sm text-amber-300">
            Copy this token now — it is shown only once and cannot be recovered.
          </p>
          <div className="flex items-center gap-2">
            <code className="min-w-0 flex-1 truncate rounded bg-zinc-950 px-2 py-1.5 font-mono text-xs text-zinc-200">
              {created.token}
            </code>
            <button
              type="button"
              onClick={copy}
              className="shrink-0 rounded-md border border-zinc-700 px-2.5 py-1.5 text-xs text-zinc-300 hover:bg-zinc-800"
            >
              {copied ? 'Copied' : 'Copy'}
            </button>
          </div>
          <button
            type="button"
            onClick={() => setCreated(null)}
            className="w-fit text-xs text-zinc-500 underline-offset-2 hover:text-zinc-300 hover:underline"
          >
            I saved it — dismiss
          </button>
        </div>
      )}

      <ErrorNote error={tokens.error ?? remove.error} />
      <div className="flex flex-col gap-2">
        {tokens.data?.map((t) => (
          <div
            key={t.uuid}
            className="flex items-center gap-3 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-2.5"
          >
            <span className="font-medium text-zinc-100">{t.name}</span>
            <span className="text-xs text-zinc-500">{t.abilities.join(', ')}</span>
            <span className="ml-auto text-xs text-zinc-500">
              {t.last_used_at ? `last used ${new Date(t.last_used_at).toLocaleString()}` : 'never used'}
            </span>
            <button
              type="button"
              className={`${btnDanger} py-1 text-xs`}
              disabled={remove.isPending}
              onClick={() => remove.mutate(t.uuid)}
            >
              Revoke
            </button>
          </div>
        ))}
        {tokens.data?.length === 0 && <p className="text-sm text-zinc-500">No API tokens.</p>}
      </div>
    </section>
  )
}

function S3Row({ storage }: { storage: S3Storage }) {
  const queryClient = useQueryClient()
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState(storage.name)
  const [region, setRegion] = useState(storage.region)
  const [endpoint, setEndpoint] = useState(storage.endpoint ?? '')
  const [bucket, setBucket] = useState(storage.bucket)
  const [path, setPath] = useState(storage.path)
  const [key, setKey] = useState('')
  const [secret, setSecret] = useState('')
  const [testResult, setTestResult] = useState<S3TestResponse | null>(null)

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ['s3-storages'] })

  const save = useMutation({
    mutationFn: () =>
      api.patch<S3Storage>(`/s3-storages/${storage.uuid}`, {
        name,
        region,
        endpoint: endpoint.trim() || null,
        bucket,
        path,
        key: key.trim() || null,
        secret: secret.trim() || null,
      }),
    onSuccess: () => {
      setEditing(false)
      setKey('')
      setSecret('')
      invalidate()
    },
  })

  const test = useMutation({
    mutationFn: () => api.post<S3TestResponse>(`/s3-storages/${storage.uuid}/test`),
    onSuccess: (r) => {
      setTestResult(r)
      invalidate()
    },
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/s3-storages/${storage.uuid}`),
    onSuccess: invalidate,
  })

  return (
    <div className="flex flex-col gap-2 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-3">
      <div className="flex items-center gap-3">
        <span className="font-medium text-zinc-100">{storage.name}</span>
        <span
          className={`flex items-center gap-1.5 text-xs ${
            storage.is_usable ? 'text-emerald-400' : 'text-zinc-500'
          }`}
        >
          <span
            className={`h-1.5 w-1.5 rounded-full ${
              storage.is_usable ? 'bg-emerald-400' : 'bg-zinc-500'
            }`}
          />
          {storage.is_usable ? 'usable' : 'untested'}
        </span>
        <span className="truncate font-mono text-xs text-zinc-500">
          {storage.bucket}
          {storage.endpoint ? ` @ ${storage.endpoint}` : ''}
        </span>
        <div className="ml-auto flex gap-2">
          <button
            type="button"
            className={`${btnGhost} py-1 text-xs`}
            disabled={test.isPending}
            onClick={() => test.mutate()}
          >
            {test.isPending ? 'Testing…' : 'Test'}
          </button>
          <button
            type="button"
            className={`${btnGhost} py-1 text-xs`}
            onClick={() => setEditing((v) => !v)}
          >
            {editing ? 'Cancel' : 'Edit'}
          </button>
          <button
            type="button"
            className={`${btnDanger} py-1 text-xs`}
            disabled={remove.isPending}
            onClick={() => remove.mutate()}
          >
            Delete
          </button>
        </div>
      </div>

      {testResult && (
        <p className={`text-xs ${testResult.usable ? 'text-emerald-400' : 'text-red-400'}`}>
          {testResult.message}
        </p>
      )}
      <ErrorNote error={test.error ?? remove.error} />

      {editing && (
        <form
          onSubmit={(e: FormEvent) => {
            e.preventDefault()
            save.mutate()
          }}
          className="mt-1 flex flex-col gap-3"
        >
          <div className="grid grid-cols-2 gap-3">
            <Field label="Name">
              <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
            </Field>
            <Field label="Region">
              <input className={inputCls} value={region} onChange={(e) => setRegion(e.target.value)} />
            </Field>
            <Field label="Endpoint">
              <input
                className={`${inputCls} font-mono`}
                value={endpoint}
                onChange={(e) => setEndpoint(e.target.value)}
              />
            </Field>
            <Field label="Bucket">
              <input className={inputCls} value={bucket} onChange={(e) => setBucket(e.target.value)} />
            </Field>
            <Field label="Path">
              <input className={inputCls} value={path} onChange={(e) => setPath(e.target.value)} />
            </Field>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Access key (leave blank to keep)">
              <input
                className={`${inputCls} font-mono`}
                value={key}
                onChange={(e) => setKey(e.target.value)}
                autoComplete="off"
              />
            </Field>
            <Field label="Secret key (leave blank to keep)">
              <input
                type="password"
                className={`${inputCls} font-mono`}
                value={secret}
                onChange={(e) => setSecret(e.target.value)}
                autoComplete="off"
              />
            </Field>
          </div>
          <ErrorNote error={save.error} />
          <button type="submit" className={`${btnPrimary} w-fit`} disabled={save.isPending}>
            {save.isPending ? 'Saving…' : 'Save'}
          </button>
        </form>
      )}
    </div>
  )
}

function S3Section() {
  const queryClient = useQueryClient()
  const [name, setName] = useState('')
  const [region, setRegion] = useState('us-east-1')
  const [endpoint, setEndpoint] = useState('')
  const [bucket, setBucket] = useState('')
  const [path, setPath] = useState('/')
  const [key, setKey] = useState('')
  const [secret, setSecret] = useState('')

  const storages = useQuery({
    queryKey: ['s3-storages'],
    queryFn: () => api.get<S3Storage[]>('/s3-storages'),
  })

  const create = useMutation({
    mutationFn: () =>
      api.post<S3Storage>('/s3-storages', {
        name,
        region,
        endpoint: endpoint.trim() || null,
        bucket,
        path,
        key,
        secret,
      }),
    onSuccess: () => {
      setName('')
      setEndpoint('')
      setBucket('')
      setPath('/')
      setKey('')
      setSecret('')
      queryClient.invalidateQueries({ queryKey: ['s3-storages'] })
    },
  })

  const canSubmit =
    name.trim() !== '' && bucket.trim() !== '' && key.trim() !== '' && secret.trim() !== '' && !create.isPending

  return (
    <section className={`${cardCls} flex max-w-2xl flex-col gap-4`}>
      <SectionTitle>S3 storages</SectionTitle>

      <form
        onSubmit={(e: FormEvent) => {
          e.preventDefault()
          if (canSubmit) create.mutate()
        }}
        className="flex flex-col gap-3"
      >
        <div className="grid grid-cols-2 gap-3">
          <Field label="Name">
            <input
              className={inputCls}
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="backups"
            />
          </Field>
          <Field label="Region">
            <input className={inputCls} value={region} onChange={(e) => setRegion(e.target.value)} />
          </Field>
          <Field label="Endpoint">
            <input
              className={`${inputCls} font-mono`}
              value={endpoint}
              onChange={(e) => setEndpoint(e.target.value)}
              placeholder="https://s3.amazonaws.com"
            />
          </Field>
          <Field label="Bucket">
            <input
              className={inputCls}
              value={bucket}
              onChange={(e) => setBucket(e.target.value)}
              placeholder="my-bucket"
            />
          </Field>
          <Field label="Path">
            <input className={inputCls} value={path} onChange={(e) => setPath(e.target.value)} />
          </Field>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Access key">
            <input
              className={`${inputCls} font-mono`}
              value={key}
              onChange={(e) => setKey(e.target.value)}
              autoComplete="off"
            />
          </Field>
          <Field label="Secret key">
            <input
              type="password"
              className={`${inputCls} font-mono`}
              value={secret}
              onChange={(e) => setSecret(e.target.value)}
              autoComplete="off"
            />
          </Field>
        </div>
        <ErrorNote error={create.error} />
        <button type="submit" className={`${btnPrimary} w-fit`} disabled={!canSubmit}>
          {create.isPending ? 'Adding…' : 'Add S3 storage'}
        </button>
      </form>

      <ErrorNote error={storages.error} />
      <div className="flex flex-col gap-2">
        {storages.data?.map((s) => <S3Row key={s.uuid} storage={s} />)}
        {storages.data?.length === 0 && <p className="text-sm text-zinc-500">No S3 storages.</p>}
      </div>
    </section>
  )
}

export default function Settings() {
  return (
    <div className="flex flex-col gap-8">
      <PageTitle>Settings</PageTitle>
      <InstanceSection />
      <S3Section />
      <TokensSection />
    </div>
  )
}
