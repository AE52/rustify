import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from 'react-router'
import {
  api,
  type AwsInstanceType,
  type AwsProvisionResponse,
  type AwsRegion,
  type CloudToken,
  type HetznerProvisionResponse,
  type PrivateKey,
} from '../../api/client'
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
  selectCls,
} from '../../components/ui'

/** Loose shape of the pass-through Hetzner metadata rows. */
interface HetznerItem {
  id: number
  name: string | null
  description?: string | null
  city?: string
  cores?: number
  memory?: number
  disk?: number
}

// ----- cloud tokens -------------------------------------------------------

function TokensCard() {
  const queryClient = useQueryClient()
  const [provider, setProvider] = useState<'hetzner' | 'aws'>('hetzner')
  const [name, setName] = useState('')
  const [token, setToken] = useState('')
  const [accessKeyId, setAccessKeyId] = useState('')
  const [secretAccessKey, setSecretAccessKey] = useState('')

  const tokens = useQuery({
    queryKey: ['cloud-tokens'],
    queryFn: () => api.get<CloudToken[]>('/cloud-tokens'),
  })
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ['cloud-tokens'] })

  const create = useMutation({
    mutationFn: () =>
      api.post<CloudToken>(
        '/cloud-tokens',
        provider === 'aws'
          ? {
              provider: 'aws',
              name: name || null,
              access_key_id: accessKeyId,
              secret_access_key: secretAccessKey,
            }
          : {
              provider: 'hetzner',
              name: name || null,
              token,
            },
      ),
    onSuccess: () => {
      setName('')
      setToken('')
      setAccessKeyId('')
      setSecretAccessKey('')
      invalidate()
    },
  })
  const remove = useMutation({
    mutationFn: (uuid: string) => api.delete(`/cloud-tokens/${uuid}`),
    onSuccess: invalidate,
  })

  const ready = provider === 'aws' ? accessKeyId && secretAccessKey : token

  return (
    <section className="flex max-w-2xl flex-col gap-3">
      <SectionTitle>Cloud provider tokens</SectionTitle>
      {tokens.data?.map((t) => (
        <div
          key={t.uuid}
          className="flex items-center gap-3 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-2.5"
        >
          <span className="flex-1 truncate text-sm text-zinc-200">{t.name || t.uuid}</span>
          <span className="text-xs text-zinc-500">{t.provider}</span>
          <button
            type="button"
            className={btnDanger}
            disabled={remove.isPending}
            onClick={() => remove.mutate(t.uuid)}
          >
            Delete
          </button>
        </div>
      ))}
      <form
        onSubmit={(e: FormEvent) => {
          e.preventDefault()
          create.mutate()
        }}
        className={`${cardCls} flex flex-wrap items-end gap-3`}
      >
        <div className="w-32">
          <Field label="Provider">
            <select
              aria-label="Token provider"
              className={inputCls}
              value={provider}
              onChange={(e) => setProvider(e.target.value as 'hetzner' | 'aws')}
            >
              <option value="hetzner">Hetzner</option>
              <option value="aws">AWS</option>
            </select>
          </Field>
        </div>
        <div className="w-40">
          <Field label="Label (optional)">
            <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
        </div>
        {provider === 'aws' ? (
          <>
            <div className="min-w-40 flex-1">
              <Field label="Access key ID">
                <input
                  type="password"
                  className={inputCls}
                  value={accessKeyId}
                  onChange={(e) => setAccessKeyId(e.target.value)}
                  autoComplete="off"
                />
              </Field>
            </div>
            <div className="min-w-40 flex-1">
              <Field label="Secret access key">
                <input
                  type="password"
                  className={inputCls}
                  value={secretAccessKey}
                  onChange={(e) => setSecretAccessKey(e.target.value)}
                  autoComplete="off"
                />
              </Field>
            </div>
          </>
        ) : (
          <div className="min-w-48 flex-1">
            <Field label="API token">
              <input
                type="password"
                className={inputCls}
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder="hcloud token"
                autoComplete="off"
              />
            </Field>
          </div>
        )}
        <button type="submit" className={btnPrimary} disabled={create.isPending || !ready}>
          {create.isPending ? 'Saving…' : 'Add token'}
        </button>
        <ErrorNote error={create.error} />
      </form>
    </section>
  )
}

// ----- provision form -----------------------------------------------------

function ProvisionCard() {
  const navigate = useNavigate()
  const [tokenUuid, setTokenUuid] = useState('')
  const [name, setName] = useState('')
  const [serverType, setServerType] = useState('')
  const [location, setLocation] = useState('')
  const [image, setImage] = useState('')
  const [keyUuid, setKeyUuid] = useState('')

  const tokens = useQuery({
    queryKey: ['cloud-tokens'],
    queryFn: () => api.get<CloudToken[]>('/cloud-tokens'),
  })
  const keys = useQuery({
    queryKey: ['private-keys'],
    queryFn: () => api.get<PrivateKey[]>('/private-keys'),
  })

  const q = `?token_uuid=${encodeURIComponent(tokenUuid)}`
  const lookupEnabled = tokenUuid !== ''
  const locations = useQuery({
    queryKey: ['hetzner', 'locations', tokenUuid],
    queryFn: () => api.get<HetznerItem[]>(`/hetzner/locations${q}`),
    enabled: lookupEnabled,
  })
  const serverTypes = useQuery({
    queryKey: ['hetzner', 'server-types', tokenUuid],
    queryFn: () => api.get<HetznerItem[]>(`/hetzner/server-types${q}`),
    enabled: lookupEnabled,
  })
  const images = useQuery({
    queryKey: ['hetzner', 'images', tokenUuid],
    queryFn: () => api.get<HetznerItem[]>(`/hetzner/images${q}`),
    enabled: lookupEnabled,
  })

  const provision = useMutation({
    mutationFn: () =>
      api.post<HetznerProvisionResponse>('/servers/provision/hetzner', {
        token_uuid: tokenUuid,
        name,
        server_type: serverType,
        location,
        image: Number(image),
        private_key_uuid: keyUuid || null,
      }),
    onSuccess: (res) => navigate(`/servers/${res.uuid}`),
  })

  const ready = tokenUuid && name && serverType && location && image
  const loading = locations.isFetching || serverTypes.isFetching || images.isFetching

  return (
    <form
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        provision.mutate()
      }}
      className={`${cardCls} flex max-w-2xl flex-col gap-4`}
    >
      <SectionTitle>Provision a Hetzner server</SectionTitle>
      <Field label="API token">
        <select
          aria-label="Cloud token"
          className={selectCls}
          value={tokenUuid}
          onChange={(e) => setTokenUuid(e.target.value)}
        >
          <option value="">Select a token…</option>
          {tokens.data
            ?.filter((t) => t.provider === 'hetzner')
            .map((t) => (
              <option key={t.uuid} value={t.uuid}>
                {t.name || t.uuid}
              </option>
            ))}
        </select>
      </Field>

      {lookupEnabled && loading && <p className="text-xs text-zinc-500">Loading Hetzner options…</p>}
      <ErrorNote error={locations.error ?? serverTypes.error ?? images.error} />

      <Field label="Name">
        <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      <div className="grid grid-cols-2 gap-3">
        <Field label="Server type">
          <select
            aria-label="Server type"
            className={selectCls}
            value={serverType}
            disabled={!lookupEnabled}
            onChange={(e) => setServerType(e.target.value)}
          >
            <option value="">Select…</option>
            {serverTypes.data?.map((s) => (
              <option key={s.id} value={s.name ?? ''}>
                {s.name}
                {s.cores ? ` — ${s.cores} vCPU / ${s.memory}GB` : ''}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Location">
          <select
            aria-label="Location"
            className={selectCls}
            value={location}
            disabled={!lookupEnabled}
            onChange={(e) => setLocation(e.target.value)}
          >
            <option value="">Select…</option>
            {locations.data?.map((l) => (
              <option key={l.id} value={l.name ?? ''}>
                {l.name}
                {l.city ? ` — ${l.city}` : ''}
              </option>
            ))}
          </select>
        </Field>
      </div>
      <Field label="Image">
        <select
          aria-label="Image"
          className={selectCls}
          value={image}
          disabled={!lookupEnabled}
          onChange={(e) => setImage(e.target.value)}
        >
          <option value="">Select…</option>
          {images.data?.map((i) => (
            <option key={i.id} value={String(i.id)}>
              {i.description || i.name}
            </option>
          ))}
        </select>
      </Field>
      <Field label="SSH key (optional — defaults to the team's first)">
        <select
          className={selectCls}
          value={keyUuid}
          onChange={(e) => setKeyUuid(e.target.value)}
        >
          <option value="">Team default</option>
          {keys.data?.map((k) => (
            <option key={k.uuid} value={k.uuid}>
              {k.name}
            </option>
          ))}
        </select>
      </Field>

      <ErrorNote error={provision.error} />
      <button type="submit" className={`${btnPrimary} w-fit`} disabled={!ready || provision.isPending}>
        {provision.isPending ? 'Provisioning…' : 'Provision server'}
      </button>
    </form>
  )
}

// ----- AWS provision form ---------------------------------------------------

function AwsProvisionCard() {
  const navigate = useNavigate()
  const [tokenUuid, setTokenUuid] = useState('')
  const [name, setName] = useState('')
  const [region, setRegion] = useState('')
  const [instanceType, setInstanceType] = useState('')
  const [count, setCount] = useState('1')
  const [keyUuid, setKeyUuid] = useState('')

  const tokens = useQuery({
    queryKey: ['cloud-tokens'],
    queryFn: () => api.get<CloudToken[]>('/cloud-tokens'),
  })
  const keys = useQuery({
    queryKey: ['private-keys'],
    queryFn: () => api.get<PrivateKey[]>('/private-keys'),
  })

  const lookupEnabled = tokenUuid !== ''
  const regions = useQuery({
    queryKey: ['aws', 'regions'],
    queryFn: () => api.get<AwsRegion[]>('/aws/regions'),
    enabled: lookupEnabled,
  })
  const instanceTypes = useQuery({
    queryKey: ['aws', 'instance-types'],
    queryFn: () => api.get<AwsInstanceType[]>('/aws/instance-types'),
    enabled: lookupEnabled,
  })

  const nodes = Math.max(1, Number(count) || 1)

  const provision = useMutation({
    mutationFn: () =>
      api.post<AwsProvisionResponse>('/servers/provision/aws', {
        token_uuid: tokenUuid,
        region,
        instance_type: instanceType,
        count: nodes,
        name,
        private_key_uuid: keyUuid || null,
      }),
    onSuccess: (res) => {
      if (res.servers.length === 1) navigate(`/servers/${res.servers[0].uuid}`)
    },
  })

  // Multi-node success: no redirect — replace the form with the cluster summary.
  const created = provision.data
  if (created && created.servers.length > 1) {
    return (
      <section className={`${cardCls} flex max-w-2xl flex-col gap-3`}>
        <SectionTitle>Docker Swarm cluster of {created.servers.length} provisioned</SectionTitle>
        {created.partial && (
          <p className="text-xs text-amber-400">
            Some nodes failed to provision — check each server below.
          </p>
        )}
        <ul className="flex flex-col gap-1">
          {created.servers.map((s) => (
            <li key={s.uuid}>
              <Link
                className="text-sm text-emerald-400 hover:underline"
                to={`/servers/${s.uuid}`}
              >
                {s.name} — {s.ip}
              </Link>
            </li>
          ))}
        </ul>
      </section>
    )
  }

  const ready = tokenUuid && name && region && instanceType
  const loading = regions.isFetching || instanceTypes.isFetching

  return (
    <form
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        provision.mutate()
      }}
      className={`${cardCls} flex max-w-2xl flex-col gap-4`}
    >
      <SectionTitle>Provision an AWS server</SectionTitle>
      <Field label="AWS token">
        <select
          aria-label="AWS cloud token"
          className={selectCls}
          value={tokenUuid}
          onChange={(e) => setTokenUuid(e.target.value)}
        >
          <option value="">Select a token…</option>
          {tokens.data
            ?.filter((t) => t.provider === 'aws')
            .map((t) => (
              <option key={t.uuid} value={t.uuid}>
                {t.name || t.uuid}
              </option>
            ))}
        </select>
      </Field>

      {lookupEnabled && loading && <p className="text-xs text-zinc-500">Loading AWS options…</p>}
      <ErrorNote error={regions.error ?? instanceTypes.error} />

      <Field label="Name">
        <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      <div className="grid grid-cols-2 gap-3">
        <Field label="Region">
          <select
            aria-label="Region"
            className={selectCls}
            value={region}
            disabled={!lookupEnabled}
            onChange={(e) => setRegion(e.target.value)}
          >
            <option value="">Select…</option>
            {regions.data?.map((r) => (
              <option key={r.name} value={r.name}>
                {r.name}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Instance type">
          <select
            aria-label="Instance type"
            className={selectCls}
            value={instanceType}
            disabled={!lookupEnabled}
            onChange={(e) => setInstanceType(e.target.value)}
          >
            <option value="">Select…</option>
            {instanceTypes.data?.map((i) => (
              <option key={i.name} value={i.name}>
                {i.name} — {i.vcpus} vCPU / {i.mem_gb}GB
              </option>
            ))}
          </select>
        </Field>
      </div>
      <Field label="Nodes">
        <input
          aria-label="Nodes"
          type="number"
          min={1}
          className={inputCls}
          value={count}
          onChange={(e) => setCount(e.target.value)}
        />
      </Field>
      {nodes >= 2 && (
        <p className="text-xs text-zinc-400">Docker Swarm cluster of {nodes} nodes</p>
      )}
      <Field label="SSH key (optional — defaults to the team's first)">
        <select
          className={selectCls}
          value={keyUuid}
          onChange={(e) => setKeyUuid(e.target.value)}
        >
          <option value="">Team default</option>
          {keys.data?.map((k) => (
            <option key={k.uuid} value={k.uuid}>
              {k.name}
            </option>
          ))}
        </select>
      </Field>

      <ErrorNote error={provision.error} />
      <button
        type="submit"
        className={`${btnPrimary} w-fit`}
        disabled={!ready || provision.isPending}
      >
        {provision.isPending ? 'Provisioning…' : 'Provision server'}
      </button>
    </form>
  )
}

export default function NewServerPage() {
  const navigate = useNavigate()
  const [mode, setMode] = useState<'hetzner' | 'aws'>('hetzner')
  return (
    <div className="flex flex-col gap-8">
      <div className="flex items-center gap-4">
        <PageTitle>New server</PageTitle>
        <button type="button" className={`${btnGhost} ml-auto`} onClick={() => navigate('/')}>
          Back
        </button>
      </div>
      <p className="max-w-2xl text-sm text-zinc-400">
        Provision a fresh server on Hetzner Cloud or AWS. After creation, rustify installs Docker
        and the proxy — follow the progress on the server page.
      </p>
      <TokensCard />
      <div className="flex gap-2">
        <button
          type="button"
          className={mode === 'hetzner' ? btnPrimary : btnGhost}
          onClick={() => setMode('hetzner')}
        >
          Provision on Hetzner
        </button>
        <button
          type="button"
          className={mode === 'aws' ? btnPrimary : btnGhost}
          onClick={() => setMode('aws')}
        >
          Provision on AWS
        </button>
      </div>
      {mode === 'hetzner' ? <ProvisionCard /> : <AwsProvisionCard />}
    </div>
  )
}
