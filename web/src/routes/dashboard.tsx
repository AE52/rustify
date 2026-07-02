import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from 'react-router'
import { api, type Application, type Project, type Server } from '../api/client'
import { StatusBadge } from '../components/StatusBadge'
import { btnPrimary, cardCls, ErrorNote, inputCls, PageTitle, SectionTitle } from '../components/ui'

function ServerCard({ server }: { server: Server }) {
  const health = server.usable
    ? { color: 'bg-emerald-400', text: 'usable' }
    : server.reachable
      ? { color: 'bg-amber-400', text: 'reachable, not usable' }
      : { color: 'bg-red-400', text: 'unreachable' }
  return (
    <Link to={`/servers/${server.uuid}`} className={`${cardCls} block hover:border-zinc-600`}>
      <div className="mb-1 flex items-center justify-between gap-2">
        <span className="truncate font-medium text-zinc-100">{server.name}</span>
        <span className="flex items-center gap-1.5 text-xs text-zinc-400">
          <span className={`h-1.5 w-1.5 rounded-full ${health.color}`} />
          {health.text}
        </span>
      </div>
      <p className="font-mono text-xs text-zinc-500">
        {server.user}@{server.ip}:{server.port}
      </p>
    </Link>
  )
}

function NewProjectForm() {
  const [name, setName] = useState('')
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const create = useMutation({
    mutationFn: () => api.post<Project>('/projects', { name }),
    onSuccess: (project) => {
      queryClient.invalidateQueries({ queryKey: ['projects'] })
      navigate(`/projects/${project.uuid}`)
    },
  })
  const submit = (e: FormEvent) => {
    e.preventDefault()
    if (name.trim()) create.mutate()
  }
  return (
    <form onSubmit={submit} className="flex items-center gap-2">
      <input
        aria-label="project name"
        className={`${inputCls} w-48`}
        placeholder="New project name"
        value={name}
        onChange={(e) => setName(e.target.value)}
      />
      <button type="submit" className={btnPrimary} disabled={!name.trim() || create.isPending}>
        Create
      </button>
      <ErrorNote error={create.error} />
    </form>
  )
}

export default function Dashboard() {
  const servers = useQuery({ queryKey: ['servers'], queryFn: () => api.get<Server[]>('/servers') })
  const projects = useQuery({ queryKey: ['projects'], queryFn: () => api.get<Project[]>('/projects') })
  const applications = useQuery({
    queryKey: ['applications'],
    queryFn: () => api.get<Application[]>('/applications'),
    refetchInterval: 15_000,
  })

  const empty = servers.data?.length === 0

  return (
    <div className="flex flex-col gap-10">
      <div className="flex items-center justify-between">
        <PageTitle>Dashboard</PageTitle>
      </div>

      {empty && (
        <div className={`${cardCls} border-dashed text-sm text-zinc-400`}>
          No servers yet.{' '}
          <Link to="/onboarding" className="font-medium text-zinc-100 underline underline-offset-2">
            Run the onboarding wizard
          </Link>{' '}
          to connect your first server and deploy an application.
        </div>
      )}

      <section>
        <SectionTitle>Servers</SectionTitle>
        <ErrorNote error={servers.error} />
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {servers.data?.map((s) => <ServerCard key={s.uuid} server={s} />)}
        </div>
        {servers.isPending && <p className="text-sm text-zinc-500">Loading…</p>}
      </section>

      <section>
        <div className="mb-3 flex items-center justify-between gap-4">
          <SectionTitle>Projects</SectionTitle>
          <NewProjectForm />
        </div>
        <ErrorNote error={projects.error} />
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {projects.data?.map((p) => (
            <Link key={p.uuid} to={`/projects/${p.uuid}`} className={`${cardCls} block hover:border-zinc-600`}>
              <span className="font-medium text-zinc-100">{p.name}</span>
              {p.description && <p className="mt-1 truncate text-xs text-zinc-500">{p.description}</p>}
            </Link>
          ))}
        </div>
        {projects.data?.length === 0 && <p className="text-sm text-zinc-500">No projects yet.</p>}
      </section>

      <section>
        <SectionTitle>Applications</SectionTitle>
        <ErrorNote error={applications.error} />
        <div className="flex flex-col gap-2">
          {applications.data?.map((a) => (
            <Link
              key={a.uuid}
              to={`/applications/${a.uuid}`}
              className="flex items-center justify-between gap-3 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-2.5 hover:border-zinc-600"
            >
              <div className="min-w-0">
                <span className="font-medium text-zinc-100">{a.name}</span>
                <span className="ml-3 truncate font-mono text-xs text-zinc-500">
                  {a.git_repository}#{a.git_branch}
                </span>
              </div>
              <StatusBadge status={a.status} />
            </Link>
          ))}
          {applications.data?.length === 0 && <p className="text-sm text-zinc-500">No applications yet.</p>}
        </div>
      </section>
    </div>
  )
}
