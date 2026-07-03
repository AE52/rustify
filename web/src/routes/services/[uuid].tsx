import { useEffect, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate, useParams } from 'react-router'
import {
  api,
  type Service,
  type ServiceApplication,
  type ServiceTemplateDetail,
} from '../../api/client'
import { ws } from '../../api/ws'
import { ConfirmDanger } from '../../components/ConfirmDanger'
import { ScheduledTasks } from '../../components/ScheduledTasks'
import { StatusBadge } from '../../components/StatusBadge'
import {
  btnGhost,
  btnPrimary,
  cardCls,
  ErrorNote,
  PageTitle,
  SectionTitle,
} from '../../components/ui'

type Tab = 'applications' | 'tasks' | 'danger'

function fqdnUrl(fqdn: string): string {
  if (fqdn.startsWith('http://') || fqdn.startsWith('https://')) return fqdn
  return `https://${fqdn}`
}

function AppCard({ app }: { app: ServiceApplication }) {
  return (
    <div className="flex flex-col gap-1.5 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-3">
      <div className="flex items-center gap-3">
        <span className="font-medium text-zinc-100">{app.name}</span>
        {app.is_database && (
          <span className="rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400">database</span>
        )}
        <StatusBadge status={app.status} />
      </div>
      {app.image && <code className="truncate font-mono text-xs text-zinc-500">{app.image}</code>}
      {app.fqdn && (
        <div className="flex flex-wrap gap-2">
          {app.fqdn.split(',').map((f) => {
            const url = fqdnUrl(f.trim())
            return (
              <a
                key={f}
                href={url}
                target="_blank"
                rel="noreferrer"
                className="truncate font-mono text-xs text-sky-400 hover:underline"
              >
                {url}
              </a>
            )
          })}
        </div>
      )}
    </div>
  )
}

function ApplicationsTab({ service }: { service: Service }) {
  const template = useQuery({
    queryKey: ['service-template', service.template_key],
    queryFn: () =>
      api.get<ServiceTemplateDetail>(`/service-templates/${service.template_key}`),
  })

  let compose = ''
  if (template.data) {
    try {
      compose = atob(template.data.compose_b64)
    } catch {
      compose = ''
    }
  }

  return (
    <div className="flex flex-col gap-6">
      <section className="flex flex-col gap-2">
        <SectionTitle>Applications</SectionTitle>
        {service.applications.map((a) => (
          <AppCard key={a.uuid} app={a} />
        ))}
        {service.applications.length === 0 && (
          <p className="text-sm text-zinc-500">No containers yet — deploy the service.</p>
        )}
      </section>

      <section className="flex max-w-3xl flex-col gap-2">
        <SectionTitle>Configuration &amp; environment</SectionTitle>
        <p className="text-xs text-zinc-500">
          Environment variables and services from the <code>{service.template_key}</code> template
          compose.
        </p>
        <ErrorNote error={template.error} />
        {compose && (
          <pre className={`${cardCls} max-h-96 overflow-auto font-mono text-xs text-zinc-300`}>
            {compose}
          </pre>
        )}
      </section>
    </div>
  )
}

export default function ServicePage() {
  const { uuid = '' } = useParams()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [tab, setTab] = useState<Tab>('applications')

  const service = useQuery({
    queryKey: ['service', uuid],
    queryFn: () => api.get<Service>(`/services/${uuid}`),
    refetchInterval: 15_000,
  })

  useEffect(() => {
    return ws.subscribe(`service:${uuid}`, (env) => {
      if (env.event === 'service_status_changed') {
        queryClient.invalidateQueries({ queryKey: ['service', uuid] })
      }
    })
  }, [uuid, queryClient])

  const lifecycle = useMutation({
    mutationFn: (action: 'deploy' | 'stop' | 'restart') =>
      api.post(`/services/${uuid}/${action}`),
    onSuccess: () => service.refetch(),
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/services/${uuid}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['services'] })
      navigate('/services')
    },
  })

  if (service.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (service.isError) return <ErrorNote error={service.error} />

  const s = service.data

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-center gap-4">
        <PageTitle>{s.name}</PageTitle>
        <StatusBadge status={s.status} />
        <span className="text-xs text-zinc-500">{s.template_key}</span>
        <div className="ml-auto flex items-center gap-2">
          <button
            type="button"
            className={btnGhost}
            disabled={lifecycle.isPending}
            onClick={() => lifecycle.mutate('restart')}
          >
            Restart
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
            className={btnPrimary}
            disabled={lifecycle.isPending}
            onClick={() => lifecycle.mutate('deploy')}
          >
            {lifecycle.isPending ? 'Working…' : 'Deploy'}
          </button>
        </div>
      </div>
      <ErrorNote error={lifecycle.error} />

      <nav className="flex gap-1 border-b border-zinc-800 text-sm">
        {(
          [
            ['applications', 'Applications'],
            ['tasks', 'Scheduled tasks'],
            ['danger', 'Danger'],
          ] as const
        ).map(([t, label]) => (
          <button
            key={t}
            type="button"
            onClick={() => setTab(t)}
            className={`-mb-px border-b-2 px-3 py-2 ${
              tab === t
                ? 'border-zinc-100 font-medium text-zinc-100'
                : 'border-transparent text-zinc-500 hover:text-zinc-300'
            }`}
          >
            {label}
          </button>
        ))}
      </nav>

      {tab === 'applications' && <ApplicationsTab service={s} />}
      {tab === 'tasks' && <ScheduledTasks resource="services" uuid={uuid} />}
      {tab === 'danger' && (
        <section className="max-w-2xl">
          <SectionTitle>Danger zone</SectionTitle>
          <ConfirmDanger
            label="Delete service"
            confirmText={s.name}
            description="Deletes this service and all its containers."
            busy={remove.isPending}
            onConfirm={() => remove.mutate()}
          />
          <ErrorNote error={remove.error} />
        </section>
      )}
    </div>
  )
}
