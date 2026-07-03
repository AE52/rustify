import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router'
import {
  api,
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
  const [name, setName] = useState('')
  const [token, setToken] = useState('')

  const tokens = useQuery({
    queryKey: ['cloud-tokens'],
    queryFn: () => api.get<CloudToken[]>('/cloud-tokens'),
  })
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ['cloud-tokens'] })

  const create = useMutation({
    mutationFn: () =>
      api.post<CloudToken>('/cloud-tokens', {
        provider: 'hetzner',
        name: name || null,
        token,
      }),
    onSuccess: () => {
      setName('')
      setToken('')
      invalidate()
    },
  })
  const remove = useMutation({
    mutationFn: (uuid: string) => api.delete(`/cloud-tokens/${uuid}`),
    onSuccess: invalidate,
  })

  return (
    <section className="flex max-w-2xl flex-col gap-3">
      <SectionTitle>Hetzner API tokens</SectionTitle>
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
        <div className="w-40">
          <Field label="Label (optional)">
            <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
        </div>
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
        <button type="submit" className={btnPrimary} disabled={create.isPending || !token}>
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
          {tokens.data?.map((t) => (
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

export default function NewServerPage() {
  const navigate = useNavigate()
  return (
    <div className="flex flex-col gap-8">
      <div className="flex items-center gap-4">
        <PageTitle>New server</PageTitle>
        <button type="button" className={`${btnGhost} ml-auto`} onClick={() => navigate('/')}>
          Back
        </button>
      </div>
      <p className="max-w-2xl text-sm text-zinc-400">
        Provision a fresh server on Hetzner Cloud. After creation, rustify installs Docker and the
        proxy — follow the progress on the server page.
      </p>
      <TokensCard />
      <ProvisionCard />
    </div>
  )
}
