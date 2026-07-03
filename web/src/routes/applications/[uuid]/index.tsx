import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient, type UseQueryResult } from '@tanstack/react-query'
import { NavLink, Outlet, useNavigate, useOutletContext, useParams } from 'react-router'
import { api, type Application, type BuildPack } from '../../../api/client'
import { ConfirmDanger } from '../../../components/ConfirmDanger'
import { StatusBadge } from '../../../components/StatusBadge'
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
} from '../../../components/ui'

export interface ApplicationContext {
  app: Application
  refetch: () => void
}

export function useApplication(): ApplicationContext {
  return useOutletContext<ApplicationContext>()
}

const TABS = [
  { to: '', label: 'General', end: true },
  { to: 'envs', label: 'Envs' },
  { to: 'storage', label: 'Storage' },
  { to: 'source', label: 'Source' },
  { to: 'domains', label: 'Domains' },
  { to: 'deployments', label: 'Deployments' },
] as const

export function ApplicationLayout() {
  const { uuid = '' } = useParams()
  const navigate = useNavigate()

  const app: UseQueryResult<Application> = useQuery({
    queryKey: ['application', uuid],
    queryFn: () => api.get<Application>(`/applications/${uuid}`),
    refetchInterval: 15_000,
  })

  const deploy = useMutation({
    mutationFn: (forceRebuild: boolean) =>
      api.post<{ deployment_uuid: string }>(`/applications/${uuid}/deploy`, {
        force_rebuild: forceRebuild,
      }),
    onSuccess: (res) => navigate(`/deployments/${res.deployment_uuid}`),
  })

  const lifecycle = useMutation({
    mutationFn: (action: 'stop' | 'restart') => api.post(`/applications/${uuid}/${action}`),
    onSuccess: () => app.refetch(),
  })

  if (app.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (app.isError) return <ErrorNote error={app.error} />

  const a = app.data

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-center gap-4">
        <PageTitle>{a.name}</PageTitle>
        <StatusBadge status={a.status} />
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
            disabled={deploy.isPending}
            onClick={() => deploy.mutate(false)}
          >
            {deploy.isPending ? 'Queueing…' : 'Deploy'}
          </button>
        </div>
      </div>
      <ErrorNote error={deploy.error ?? lifecycle.error} />

      <nav className="flex gap-1 border-b border-zinc-800 text-sm">
        {TABS.map((t) => (
          <NavLink
            key={t.to}
            to={t.to}
            end={'end' in t && t.end}
            className={({ isActive }) =>
              `-mb-px border-b-2 px-3 py-2 ${
                isActive
                  ? 'border-zinc-100 font-medium text-zinc-100'
                  : 'border-transparent text-zinc-500 hover:text-zinc-300'
              }`
            }
          >
            {t.label}
          </NavLink>
        ))}
      </nav>

      <Outlet context={{ app: a, refetch: () => app.refetch() } satisfies ApplicationContext} />
    </div>
  )
}

/** General tab: name, runtime knobs, lifecycle danger zone. */
export default function ApplicationGeneral() {
  const { app, refetch } = useApplication()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [name, setName] = useState(app.name)
  const [buildPack, setBuildPack] = useState<BuildPack>(app.build_pack as BuildPack)
  const [portsExposes, setPortsExposes] = useState(app.ports_exposes)
  const [portsMappings, setPortsMappings] = useState(app.ports_mappings ?? '')
  const [limitsMemory, setLimitsMemory] = useState(app.limits_memory ?? '0')
  const [limitsCpus, setLimitsCpus] = useState(app.limits_cpus ?? '0')
  const [healthEnabled, setHealthEnabled] = useState(app.health_check_enabled ?? false)
  const [healthPath, setHealthPath] = useState(app.health_check_path ?? '/')

  const save = useMutation({
    mutationFn: () =>
      api.patch<Application>(`/applications/${app.uuid}`, {
        name,
        build_pack: buildPack,
        ports_exposes: portsExposes,
        ports_mappings: portsMappings || null,
        limits_memory: limitsMemory,
        limits_cpus: limitsCpus,
        health_check_enabled: healthEnabled,
        health_check_path: healthPath,
      }),
    onSuccess: () => refetch(),
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/applications/${app.uuid}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['applications'] })
      navigate('/')
    },
  })

  const submit = (e: FormEvent) => {
    e.preventDefault()
    save.mutate()
  }

  const buildPacks: BuildPack[] = ['nixpacks', 'dockerfile', 'static', 'docker_image', 'docker_compose']

  return (
    <div className="flex flex-col gap-8">
      <form onSubmit={submit} className={`${cardCls} flex max-w-2xl flex-col gap-4`}>
        <SectionTitle>General</SectionTitle>
        <Field label="Name">
          <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <div className="grid grid-cols-3 gap-3">
          <Field label="Build pack">
            <select className={selectCls} value={buildPack} onChange={(e) => setBuildPack(e.target.value as BuildPack)}>
              {buildPacks.map((bp) => (
                <option key={bp} value={bp}>
                  {bp}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Exposed port(s)">
            <input className={inputCls} value={portsExposes} onChange={(e) => setPortsExposes(e.target.value)} />
          </Field>
          <Field label="Port mappings">
            <input
              className={inputCls}
              value={portsMappings}
              onChange={(e) => setPortsMappings(e.target.value)}
              placeholder="8080:80"
            />
          </Field>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Memory limit (0 = unlimited)">
            <input className={inputCls} value={limitsMemory} onChange={(e) => setLimitsMemory(e.target.value)} />
          </Field>
          <Field label="CPU limit (0 = unlimited)">
            <input className={inputCls} value={limitsCpus} onChange={(e) => setLimitsCpus(e.target.value)} />
          </Field>
        </div>
        <div className="flex items-end gap-3">
          <label className="flex items-center gap-2 pb-1.5 text-sm text-zinc-300">
            <input
              type="checkbox"
              checked={healthEnabled}
              onChange={(e) => setHealthEnabled(e.target.checked)}
              className="accent-zinc-400"
            />
            Health check
          </label>
          <div className="flex-1">
            <Field label="Health check path">
              <input
                className={inputCls}
                value={healthPath}
                onChange={(e) => setHealthPath(e.target.value)}
                disabled={!healthEnabled}
              />
            </Field>
          </div>
        </div>
        <ErrorNote error={save.error} />
        <button type="submit" className={`${btnPrimary} w-fit`} disabled={save.isPending}>
          {save.isPending ? 'Saving…' : 'Save'}
        </button>
      </form>

      <section className="max-w-2xl">
        <SectionTitle>Danger zone</SectionTitle>
        <ConfirmDanger
          label="Delete application"
          confirmText={app.name}
          description="Deletes this application and its deployment history."
          busy={remove.isPending}
          onConfirm={() => remove.mutate()}
        />
        <ErrorNote error={remove.error} />
      </section>
    </div>
  )
}
