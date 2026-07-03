import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate, useParams } from 'react-router'
import { api, type PrivateKey, type ProxyConfig, type Server } from '../../api/client'
import { ws } from '../../api/ws'
import { ConfirmDanger } from '../../components/ConfirmDanger'
import { StatusBadge } from '../../components/StatusBadge'
import { Terminal } from '../../components/Terminal'
import { hostTarget } from '../../lib/terminal'
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

type Tab = 'general' | 'proxy' | 'terminal'

function GeneralTab({ server }: { server: Server }) {
  const [name, setName] = useState(server.name)
  const [ip, setIp] = useState(server.ip)
  const [port, setPort] = useState(String(server.port))
  const [user, setUser] = useState(server.user)
  const [keyUuid, setKeyUuid] = useState(server.private_key_uuid)
  const [validationLines, setValidationLines] = useState<string[]>([])
  const outputRef = useRef<HTMLDivElement>(null)
  const queryClient = useQueryClient()
  const navigate = useNavigate()

  const keys = useQuery({
    queryKey: ['private-keys'],
    queryFn: () => api.get<PrivateKey[]>('/private-keys'),
  })

  // Validation output streams on WS channel `server:<uuid>` (C5).
  useEffect(() => {
    return ws.subscribe(`server:${server.uuid}`, (env) => {
      if (env.event === 'server_reachability_changed') {
        queryClient.invalidateQueries({ queryKey: ['server', server.uuid] })
      }
      const data = env.data as Record<string, unknown> | string | null
      const text =
        typeof data === 'string'
          ? data
          : data && typeof data === 'object'
            ? String((data.content as string | undefined) ?? (data.line as string | undefined) ?? JSON.stringify(data))
            : env.event
      setValidationLines((prev) => [...prev, text])
    })
  }, [server.uuid, queryClient])

  useEffect(() => {
    const el = outputRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [validationLines])

  const save = useMutation({
    mutationFn: () =>
      api.patch<Server>(`/servers/${server.uuid}`, {
        name,
        ip,
        port: Number(port) || 22,
        user,
        private_key_uuid: keyUuid,
      }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['server', server.uuid] }),
  })

  const validate = useMutation({
    mutationFn: () => api.post<{ job_uuid: string }>(`/servers/${server.uuid}/validate`),
    onSuccess: () => setValidationLines((prev) => [...prev, 'validation started…']),
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/servers/${server.uuid}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['servers'] })
      navigate('/')
    },
  })

  const submit = (e: FormEvent) => {
    e.preventDefault()
    save.mutate()
  }

  return (
    <div className="flex flex-col gap-8">
      <form onSubmit={submit} className={`${cardCls} flex max-w-2xl flex-col gap-4`}>
        <SectionTitle>Connection</SectionTitle>
        <Field label="Name">
          <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <div className="grid grid-cols-3 gap-3">
          <div className="col-span-2">
            <Field label="IP address">
              <input className={inputCls} value={ip} onChange={(e) => setIp(e.target.value)} />
            </Field>
          </div>
          <Field label="Port">
            <input className={inputCls} value={port} onChange={(e) => setPort(e.target.value)} />
          </Field>
        </div>
        <Field label="User">
          <input className={inputCls} value={user} onChange={(e) => setUser(e.target.value)} />
        </Field>
        <Field label="Private key">
          <select className={selectCls} value={keyUuid} onChange={(e) => setKeyUuid(e.target.value)}>
            {keys.data?.map((k) => (
              <option key={k.uuid} value={k.uuid}>
                {k.name}
              </option>
            ))}
          </select>
        </Field>
        <ErrorNote error={save.error} />
        <div className="flex gap-2">
          <button type="submit" className={btnPrimary} disabled={save.isPending}>
            {save.isPending ? 'Saving…' : 'Save'}
          </button>
          <button type="button" className={btnGhost} disabled={validate.isPending} onClick={() => validate.mutate()}>
            Validate connection
          </button>
        </div>
        <ErrorNote error={validate.error} />
      </form>

      <section>
        <SectionTitle>Validation output</SectionTitle>
        <div
          ref={outputRef}
          role="log"
          aria-label="validation output"
          className="h-56 overflow-auto rounded-lg border border-zinc-800 bg-zinc-950 p-3 font-mono text-xs text-zinc-300"
        >
          {validationLines.length === 0 ? (
            <span className="text-zinc-600">
              {server.validation_logs ?? 'Run a validation to stream output here.'}
            </span>
          ) : (
            validationLines.map((l, i) => <div key={i}>{l}</div>)
          )}
        </div>
      </section>

      <section className="max-w-2xl">
        <SectionTitle>Danger zone</SectionTitle>
        <ConfirmDanger
          label="Delete server"
          confirmText={server.name}
          description="Deletes this server and its settings from rustify."
          busy={remove.isPending}
          onConfirm={() => remove.mutate()}
        />
        <ErrorNote error={remove.error} />
      </section>
    </div>
  )
}

function ProxyTab({ serverUuid }: { serverUuid: string }) {
  const queryClient = useQueryClient()
  const proxy = useQuery({
    queryKey: ['server', serverUuid, 'proxy'],
    queryFn: () => api.get<ProxyConfig>(`/servers/${serverUuid}/proxy`),
  })
  const [config, setConfig] = useState<string | null>(null)

  const save = useMutation({
    mutationFn: () => api.patch<ProxyConfig>(`/servers/${serverUuid}/proxy`, { proxy_custom_config: config }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['server', serverUuid, 'proxy'] }),
  })

  const lifecycle = useMutation({
    mutationFn: (action: 'start' | 'stop' | 'restart') =>
      api.post(`/servers/${serverUuid}/proxy/${action}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['server', serverUuid, 'proxy'] }),
  })

  if (proxy.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (proxy.isError) return <ErrorNote error={proxy.error} />

  const value = config ?? proxy.data.proxy_custom_config ?? ''

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center gap-3">
        <span className="text-sm text-zinc-400">
          Proxy: <span className="font-mono text-zinc-200">{proxy.data.proxy_type}</span>
        </span>
        <StatusBadge status={proxy.data.proxy_status} />
        <div className="ml-auto flex gap-2">
          <button
            type="button"
            className={btnGhost}
            disabled={lifecycle.isPending}
            onClick={() => lifecycle.mutate('start')}
          >
            Start
          </button>
          <button
            type="button"
            className={btnGhost}
            disabled={lifecycle.isPending}
            onClick={() => lifecycle.mutate('stop')}
          >
            Stop
          </button>
          <button
            type="button"
            className={btnGhost}
            disabled={lifecycle.isPending}
            onClick={() => lifecycle.mutate('restart')}
          >
            Restart
          </button>
        </div>
      </div>
      <ErrorNote error={lifecycle.error} />
      <Field label="Custom proxy configuration (docker compose)">
        <textarea
          aria-label="proxy config"
          className={`${inputCls} h-80 font-mono text-xs`}
          value={value}
          onChange={(e) => setConfig(e.target.value)}
          placeholder="# custom traefik compose overrides"
          spellCheck={false}
        />
      </Field>
      <ErrorNote error={save.error} />
      <button
        type="button"
        className={`${btnPrimary} w-fit`}
        disabled={save.isPending || config === null}
        onClick={() => save.mutate()}
      >
        {save.isPending ? 'Saving…' : 'Save configuration'}
      </button>
    </div>
  )
}

export default function ServerPage() {
  const { uuid = '' } = useParams()
  const [tab, setTab] = useState<Tab>('general')

  const server = useQuery({
    queryKey: ['server', uuid],
    queryFn: () => api.get<Server>(`/servers/${uuid}`),
  })

  if (server.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (server.isError) return <ErrorNote error={server.error} />

  const s = server.data

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center gap-4">
        <PageTitle>{s.name}</PageTitle>
        <span className="flex items-center gap-1.5 text-xs text-zinc-400">
          <span
            className={`h-1.5 w-1.5 rounded-full ${
              s.usable ? 'bg-emerald-400' : s.reachable ? 'bg-amber-400' : 'bg-red-400'
            }`}
          />
          {s.usable ? 'usable' : s.reachable ? 'reachable' : 'unreachable'}
        </span>
        <span className="font-mono text-xs text-zinc-500">
          {s.user}@{s.ip}:{s.port}
        </span>
      </div>

      <div className="flex gap-1 border-b border-zinc-800 text-sm">
        {(['general', 'proxy', 'terminal'] as const).map((t) => (
          <button
            key={t}
            type="button"
            onClick={() => setTab(t)}
            className={`-mb-px border-b-2 px-3 py-2 capitalize ${
              tab === t
                ? 'border-zinc-100 font-medium text-zinc-100'
                : 'border-transparent text-zinc-500 hover:text-zinc-300'
            }`}
          >
            {t}
          </button>
        ))}
      </div>

      {tab === 'general' ? (
        <GeneralTab server={s} />
      ) : tab === 'proxy' ? (
        <ProxyTab serverUuid={uuid} />
      ) : (
        <section className="flex flex-col gap-3">
          <SectionTitle>Terminal</SectionTitle>
          <p className="text-xs text-zinc-500">
            An SSH shell on <span className="font-mono text-zinc-400">{s.user}@{s.ip}</span>. Admins
            and owners only; sessions expire after 8 hours.
          </p>
          <Terminal target={hostTarget(uuid)} />
        </section>
      )}
    </div>
  )
}
