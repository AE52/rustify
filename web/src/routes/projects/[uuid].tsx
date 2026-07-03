import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router'
import {
  api,
  type Application,
  type BranchesResponse,
  type BuildPack,
  type Environment,
  type GithubApp,
  type PrivateKey,
  type Project,
  type RepositoriesResponse,
  type Server,
} from '../../api/client'
import { parseBranch, parseRepo, type GithubRepo } from '../../lib/github'
import { ConfirmDanger } from '../../components/ConfirmDanger'
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

const BUILD_PACKS: BuildPack[] = ['nixpacks', 'dockerfile', 'static', 'docker_image', 'docker_compose', 'railpack']

type SourceKind = 'public' | 'github_app' | 'deploy_key'

/** GitHub App source: pick app -> repo -> branch. Reports selection upward. */
function GithubAppSource({
  onSelect,
}: {
  onSelect: (v: { appUuid: string; repo: GithubRepo | null; branch: string }) => void
}) {
  const [appUuid, setAppUuid] = useState('')
  const [repo, setRepo] = useState<GithubRepo | null>(null)
  const [branch, setBranch] = useState('')

  const apps = useQuery({ queryKey: ['github-apps'], queryFn: () => api.get<GithubApp[]>('/github-apps') })
  const repos = useQuery({
    queryKey: ['github-app', appUuid, 'repositories'],
    queryFn: () => api.get<RepositoriesResponse>(`/github-apps/${appUuid}/repositories`),
    enabled: Boolean(appUuid),
  })
  const parsedRepos = (repos.data?.repositories ?? [])
    .map(parseRepo)
    .filter((r): r is GithubRepo => r !== null)

  const branches = useQuery({
    queryKey: ['github-app', appUuid, 'branches', repo?.full_name],
    queryFn: () =>
      api.get<BranchesResponse>(`/github-apps/${appUuid}/repositories/${repo!.owner}/${repo!.name}/branches`),
    enabled: Boolean(appUuid && repo),
  })
  const parsedBranches = (branches.data?.branches ?? [])
    .map(parseBranch)
    .filter((b): b is string => b !== null)

  return (
    <>
      <Field label="GitHub App">
        <select
          aria-label="GitHub App"
          className={selectCls}
          value={appUuid}
          onChange={(e) => {
            setAppUuid(e.target.value)
            setRepo(null)
            setBranch('')
            onSelect({ appUuid: e.target.value, repo: null, branch: '' })
          }}
        >
          <option value="">— select an app —</option>
          {apps.data?.map((a) => (
            <option key={a.uuid} value={a.uuid}>
              {a.name}
            </option>
          ))}
        </select>
      </Field>
      {appUuid && (
        <Field label="Repository">
          <select
            aria-label="Repository"
            className={selectCls}
            value={repo?.full_name ?? ''}
            onChange={(e) => {
              const r = parsedRepos.find((x) => x.full_name === e.target.value) ?? null
              const b = r?.default_branch ?? ''
              setRepo(r)
              setBranch(b)
              onSelect({ appUuid, repo: r, branch: b })
            }}
          >
            <option value="">— select a repository —</option>
            {parsedRepos.map((r) => (
              <option key={r.id || r.full_name} value={r.full_name}>
                {r.full_name}
                {r.private ? ' (private)' : ''}
              </option>
            ))}
          </select>
        </Field>
      )}
      {repo && (
        <Field label="Branch">
          <select
            aria-label="Branch"
            className={selectCls}
            value={branch}
            onChange={(e) => {
              setBranch(e.target.value)
              onSelect({ appUuid, repo, branch: e.target.value })
            }}
          >
            {parsedBranches.map((b) => (
              <option key={b} value={b}>
                {b}
              </option>
            ))}
          </select>
        </Field>
      )}
    </>
  )
}

export function NewAppForm({
  projectUuid,
  environmentName,
  onCreated,
}: {
  projectUuid: string
  environmentName: string
  onCreated: (app: Application) => void
}) {
  const [name, setName] = useState('')
  const [sourceKind, setSourceKind] = useState<SourceKind>('public')
  const [repo, setRepo] = useState('')
  const [branch, setBranch] = useState('main')
  const [buildPack, setBuildPack] = useState<BuildPack>('nixpacks')
  const [ports, setPorts] = useState('80')
  const [serverUuid, setServerUuid] = useState('')
  const [privateKeyUuid, setPrivateKeyUuid] = useState('')
  const [ghSel, setGhSel] = useState<{ appUuid: string; repo: GithubRepo | null; branch: string }>({
    appUuid: '',
    repo: null,
    branch: '',
  })

  const servers = useQuery({ queryKey: ['servers'], queryFn: () => api.get<Server[]>('/servers') })
  const keys = useQuery({
    queryKey: ['private-keys'],
    queryFn: () => api.get<PrivateKey[]>('/private-keys'),
    enabled: sourceKind === 'deploy_key',
  })
  const selectedServer = serverUuid || servers.data?.[0]?.uuid || ''

  const create = useMutation({
    mutationFn: () => {
      const common = {
        project_uuid: projectUuid,
        environment_name: environmentName,
        server_uuid: selectedServer,
        name,
        build_pack: buildPack,
        ports_exposes: ports,
      }
      if (sourceKind === 'github_app') {
        return api.post<Application>('/applications', {
          ...common,
          source: 'github_app',
          github_app_uuid: ghSel.appUuid,
          git_repository: ghSel.repo?.full_name ?? '',
          git_branch: ghSel.branch,
          is_private: ghSel.repo?.private ?? true,
        })
      }
      if (sourceKind === 'deploy_key') {
        return api.post<Application>('/applications', {
          ...common,
          private_key_uuid: privateKeyUuid,
          git_repository: repo,
          git_branch: branch,
        })
      }
      return api.post<Application>('/applications', {
        ...common,
        git_repository: repo,
        git_branch: branch,
      })
    },
    onSuccess: onCreated,
  })

  const submit = (e: FormEvent) => {
    e.preventDefault()
    create.mutate()
  }

  const sourceReady =
    sourceKind === 'github_app'
      ? Boolean(ghSel.appUuid && ghSel.repo && ghSel.branch)
      : sourceKind === 'deploy_key'
        ? Boolean(privateKeyUuid && repo.trim())
        : repo.trim() !== ''

  return (
    <form onSubmit={submit} className={`${cardCls} flex flex-col gap-4`}>
      <SectionTitle>New application</SectionTitle>
      <div className="grid grid-cols-2 gap-3">
        <Field label="Name">
          <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
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

      <Field label="Source">
        <select
          aria-label="Source"
          className={selectCls}
          value={sourceKind}
          onChange={(e) => setSourceKind(e.target.value as SourceKind)}
        >
          <option value="public">Public Git URL</option>
          <option value="github_app">GitHub App</option>
          <option value="deploy_key">Deploy Key (private SSH)</option>
        </select>
      </Field>

      {sourceKind === 'github_app' ? (
        <GithubAppSource onSelect={setGhSel} />
      ) : (
        <>
          {sourceKind === 'deploy_key' && (
            <Field label="Private key">
              <select
                aria-label="Private key"
                className={selectCls}
                value={privateKeyUuid}
                onChange={(e) => setPrivateKeyUuid(e.target.value)}
              >
                <option value="">— select a key —</option>
                {keys.data?.map((k) => (
                  <option key={k.uuid} value={k.uuid}>
                    {k.name}
                  </option>
                ))}
              </select>
            </Field>
          )}
          <Field label="Git repository">
            <input
              className={inputCls}
              value={repo}
              onChange={(e) => setRepo(e.target.value)}
              placeholder={
                sourceKind === 'deploy_key'
                  ? 'git@github.com:acme/app.git'
                  : 'https://github.com/acme/app.git'
              }
            />
          </Field>
          <Field label="Branch">
            <input className={inputCls} value={branch} onChange={(e) => setBranch(e.target.value)} />
          </Field>
        </>
      )}

      <div className="grid grid-cols-2 gap-3">
        <Field label="Build pack">
          <select className={selectCls} value={buildPack} onChange={(e) => setBuildPack(e.target.value as BuildPack)}>
            {BUILD_PACKS.map((bp) => (
              <option key={bp} value={bp}>
                {bp}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Exposed port(s)">
          <input className={inputCls} value={ports} onChange={(e) => setPorts(e.target.value)} />
        </Field>
      </div>
      <ErrorNote error={create.error} />
      <button
        type="submit"
        className={`${btnPrimary} w-fit`}
        disabled={create.isPending || !name.trim() || !sourceReady || !selectedServer}
      >
        {create.isPending ? 'Creating…' : 'Create application'}
      </button>
    </form>
  )
}

export default function ProjectPage() {
  const { uuid = '' } = useParams()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [selectedEnv, setSelectedEnv] = useState<string | null>(null)
  const [newEnvName, setNewEnvName] = useState('')
  const [showNewApp, setShowNewApp] = useState(false)

  const project = useQuery({
    queryKey: ['project', uuid],
    queryFn: () => api.get<Project>(`/projects/${uuid}`),
  })
  const environments = useQuery({
    queryKey: ['project', uuid, 'environments'],
    queryFn: () => api.get<Environment[]>(`/projects/${uuid}/environments`),
  })

  const envs = environments.data ?? []
  const activeEnv =
    envs.find((e) => e.uuid === selectedEnv) ?? envs.find((e) => e.name === 'production') ?? envs[0]

  const applications = useQuery({
    queryKey: ['applications', { environment: activeEnv?.uuid }],
    queryFn: () => api.get<Application[]>(`/applications?environment_uuid=${activeEnv?.uuid}`),
    enabled: Boolean(activeEnv),
  })

  const createEnv = useMutation({
    mutationFn: () => api.post<Environment>(`/projects/${uuid}/environments`, { name: newEnvName }),
    onSuccess: (env) => {
      setNewEnvName('')
      setSelectedEnv(env.uuid)
      queryClient.invalidateQueries({ queryKey: ['project', uuid, 'environments'] })
    },
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/projects/${uuid}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['projects'] })
      navigate('/')
    },
  })

  if (project.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (project.isError) return <ErrorNote error={project.error} />

  const p = project.data

  return (
    <div className="flex flex-col gap-6">
      <div>
        <PageTitle>{p.name}</PageTitle>
        {p.description && <p className="mt-1 text-sm text-zinc-500">{p.description}</p>}
      </div>

      <div className="flex flex-wrap items-center gap-1 border-b border-zinc-800 pb-px text-sm">
        {envs.map((env) => (
          <button
            key={env.uuid}
            type="button"
            onClick={() => setSelectedEnv(env.uuid)}
            className={`-mb-px border-b-2 px-3 py-2 ${
              activeEnv?.uuid === env.uuid
                ? 'border-zinc-100 font-medium text-zinc-100'
                : 'border-transparent text-zinc-500 hover:text-zinc-300'
            }`}
          >
            {env.name}
          </button>
        ))}
        <form
          onSubmit={(e) => {
            e.preventDefault()
            if (newEnvName.trim()) createEnv.mutate()
          }}
          className="ml-auto flex items-center gap-2 py-1"
        >
          <input
            aria-label="new environment name"
            className={`${inputCls} w-36 py-1 text-xs`}
            placeholder="new environment"
            value={newEnvName}
            onChange={(e) => setNewEnvName(e.target.value)}
          />
          <button type="submit" className={`${btnGhost} py-1 text-xs`} disabled={!newEnvName.trim() || createEnv.isPending}>
            Add
          </button>
        </form>
      </div>
      <ErrorNote error={createEnv.error ?? environments.error} />

      <section className="flex flex-col gap-3">
        <div className="flex items-center justify-between">
          <SectionTitle>
            Applications{activeEnv ? ` — ${activeEnv.name}` : ''}
          </SectionTitle>
          <button type="button" className={btnGhost} onClick={() => setShowNewApp((v) => !v)}>
            {showNewApp ? 'Close' : 'New application'}
          </button>
        </div>

        {showNewApp && activeEnv && (
          <NewAppForm
            projectUuid={uuid}
            environmentName={activeEnv.name}
            onCreated={(app) => {
              queryClient.invalidateQueries({ queryKey: ['applications'] })
              navigate(`/applications/${app.uuid}`)
            }}
          />
        )}

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
          {applications.data?.length === 0 && (
            <p className="text-sm text-zinc-500">No applications in this environment.</p>
          )}
        </div>
      </section>

      <section className="max-w-2xl">
        <SectionTitle>Danger zone</SectionTitle>
        <ConfirmDanger
          label="Delete project"
          confirmText={p.name}
          description="Deletes this project, its environments and applications."
          busy={remove.isPending}
          onConfirm={() => remove.mutate()}
        />
        <ErrorNote error={remove.error} />
      </section>
    </div>
  )
}
