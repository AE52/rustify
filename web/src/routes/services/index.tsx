import { useMemo, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from 'react-router'
import {
  api,
  type Environment,
  type Project,
  type Server,
  type Service,
  type ServiceTemplate,
} from '../../api/client'
import { StatusBadge } from '../../components/StatusBadge'
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

// ----- deploy-from-template flow -----------------------------------------

function DeployForm({
  template,
  onClose,
}: {
  template: ServiceTemplate
  onClose: () => void
}) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [name, setName] = useState(template.key)
  const [projectUuid, setProjectUuid] = useState('')
  const [environmentName, setEnvironmentName] = useState('')
  const [serverUuid, setServerUuid] = useState('')

  const projects = useQuery({ queryKey: ['projects'], queryFn: () => api.get<Project[]>('/projects') })
  const servers = useQuery({ queryKey: ['servers'], queryFn: () => api.get<Server[]>('/servers') })

  const selectedProject = projectUuid || projects.data?.[0]?.uuid || ''
  const environments = useQuery({
    queryKey: ['project', selectedProject, 'environments'],
    queryFn: () => api.get<Environment[]>(`/projects/${selectedProject}/environments`),
    enabled: Boolean(selectedProject),
  })
  const selectedEnv =
    environmentName ||
    environments.data?.find((e) => e.name === 'production')?.name ||
    environments.data?.[0]?.name ||
    ''
  const selectedServer = serverUuid || servers.data?.[0]?.uuid || ''

  const deploy = useMutation({
    mutationFn: () =>
      api.post<Service>('/services', {
        project_uuid: selectedProject,
        environment_name: selectedEnv,
        server_uuid: selectedServer,
        template_key: template.key,
        name,
      }),
    onSuccess: (svc) => {
      queryClient.invalidateQueries({ queryKey: ['services'] })
      navigate(`/services/${svc.uuid}`)
    },
  })

  const canSubmit =
    name.trim() !== '' && Boolean(selectedProject) && Boolean(selectedEnv) && Boolean(selectedServer) && !deploy.isPending

  return (
    <form
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        if (canSubmit) deploy.mutate()
      }}
      className={`${cardCls} flex flex-col gap-4`}
    >
      <SectionTitle>Deploy {template.name}</SectionTitle>
      <Field label="Name">
        <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      <div className="grid grid-cols-3 gap-3">
        <Field label="Project">
          <select
            className={selectCls}
            value={selectedProject}
            onChange={(e) => {
              setProjectUuid(e.target.value)
              setEnvironmentName('')
            }}
          >
            {projects.data?.map((p) => (
              <option key={p.uuid} value={p.uuid}>
                {p.name}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Environment">
          <select
            className={selectCls}
            value={selectedEnv}
            onChange={(e) => setEnvironmentName(e.target.value)}
          >
            {environments.data?.map((env) => (
              <option key={env.uuid} value={env.name}>
                {env.name}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Server">
          <select className={selectCls} value={selectedServer} onChange={(e) => setServerUuid(e.target.value)}>
            {servers.data?.map((s) => (
              <option key={s.uuid} value={s.uuid}>
                {s.name}
              </option>
            ))}
          </select>
        </Field>
      </div>
      <ErrorNote error={deploy.error} />
      <div className="flex gap-2">
        <button type="submit" className={btnPrimary} disabled={!canSubmit}>
          {deploy.isPending ? 'Deploying…' : 'Deploy service'}
        </button>
        <button type="button" className={btnGhost} onClick={onClose}>
          Cancel
        </button>
      </div>
    </form>
  )
}

function TemplateCard({
  template,
  onDeploy,
}: {
  template: ServiceTemplate
  onDeploy: () => void
}) {
  return (
    <div data-testid="template-card" className={`${cardCls} flex flex-col gap-2`}>
      <div className="flex items-center gap-2">
        {template.logo && (
          <img src={template.logo} alt="" className="h-6 w-6 shrink-0 rounded" />
        )}
        <span className="truncate font-medium text-zinc-100">{template.name}</span>
      </div>
      <p className="line-clamp-2 min-h-8 text-xs text-zinc-500">{template.slogan}</p>
      <div className="flex flex-wrap gap-1">
        {template.tags.slice(0, 4).map((t) => (
          <span key={t} className="rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400">
            {t}
          </span>
        ))}
      </div>
      <button type="button" className={`${btnGhost} mt-1 py-1 text-xs`} onClick={onDeploy}>
        Deploy
      </button>
    </div>
  )
}

// ----- page ---------------------------------------------------------------

export default function ServicesCatalog() {
  const [search, setSearch] = useState('')
  const [category, setCategory] = useState('')
  const [selected, setSelected] = useState<ServiceTemplate | null>(null)

  const templates = useQuery({
    queryKey: ['service-templates'],
    queryFn: () => api.get<ServiceTemplate[]>('/service-templates'),
  })
  const services = useQuery({
    queryKey: ['services'],
    queryFn: () => api.get<Service[]>('/services'),
    refetchInterval: 15_000,
  })

  const categories = useMemo(() => {
    const set = new Set<string>()
    for (const t of templates.data ?? []) {
      if (t.category) set.add(t.category)
    }
    return [...set].sort()
  }, [templates.data])

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase()
    return (templates.data ?? []).filter((t) => {
      if (category && t.category !== category) return false
      if (!q) return true
      return (
        t.name.toLowerCase().includes(q) ||
        t.slogan.toLowerCase().includes(q) ||
        t.tags.some((tag) => tag.toLowerCase().includes(q))
      )
    })
  }, [templates.data, search, category])

  return (
    <div className="flex flex-col gap-8">
      <PageTitle>Services</PageTitle>

      {services.data && services.data.length > 0 && (
        <section className="flex flex-col gap-2">
          <SectionTitle>Deployed services</SectionTitle>
          {services.data.map((s) => (
            <Link
              key={s.uuid}
              to={`/services/${s.uuid}`}
              className="flex items-center justify-between gap-3 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-2.5 hover:border-zinc-600"
            >
              <div className="min-w-0">
                <span className="font-medium text-zinc-100">{s.name}</span>
                <span className="ml-3 text-xs text-zinc-500">{s.template_key}</span>
              </div>
              <StatusBadge status={s.status} />
            </Link>
          ))}
        </section>
      )}

      <section className="flex flex-col gap-4">
        <SectionTitle>Service catalog</SectionTitle>
        <div className="flex flex-wrap items-end gap-3">
          <div className="flex-1">
            <Field label="Search">
              <input
                className={inputCls}
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Search services…"
                aria-label="search services"
              />
            </Field>
          </div>
          <Field label="Category">
            <select
              className={selectCls}
              value={category}
              onChange={(e) => setCategory(e.target.value)}
              aria-label="category filter"
            >
              <option value="">All categories</option>
              {categories.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          </Field>
        </div>

        {selected && <DeployForm template={selected} onClose={() => setSelected(null)} />}

        <ErrorNote error={templates.error} />
        {templates.isPending && <p className="text-sm text-zinc-500">Loading…</p>}
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {filtered.map((t) => (
            <TemplateCard key={t.key} template={t} onDeploy={() => setSelected(t)} />
          ))}
        </div>
        {templates.data && filtered.length === 0 && (
          <p className="text-sm text-zinc-500">No templates match your filters.</p>
        )}
      </section>
    </div>
  )
}
