import { useEffect, useRef, useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router'
import {
  api,
  type Application,
  type BuildPack,
  type PrivateKey,
  type Project,
  type Server,
} from '../api/client'
import { ws } from '../api/ws'
import { Wizard } from '../components/Wizard'
import {
  btnGhost,
  btnPrimary,
  cardCls,
  ErrorNote,
  Field,
  inputCls,
  PageTitle,
  selectCls,
} from '../components/ui'

// ---------------------------------------------------------------------------
// State machine: welcome → key → server → validate → project → app → deploy
// ---------------------------------------------------------------------------

export const ONBOARDING_STEPS = [
  'welcome',
  'key',
  'server',
  'validate',
  'project',
  'app',
  'deploy',
] as const

export type OnboardingStepId = (typeof ONBOARDING_STEPS)[number]

export interface OnboardingState {
  privateKeyUuid?: string
  publicKey?: string
  serverUuid?: string
  serverName?: string
  serverValidated?: boolean
  projectUuid?: string
  projectName?: string
  applicationUuid?: string
  deploymentUuid?: string
}

/** A step may only be left once its required resource exists. */
export function canLeaveStep(step: OnboardingStepId, s: OnboardingState): boolean {
  switch (step) {
    case 'welcome':
      return true
    case 'key':
      return Boolean(s.privateKeyUuid)
    case 'server':
      return Boolean(s.serverUuid)
    case 'validate':
      return Boolean(s.serverValidated)
    case 'project':
      return Boolean(s.projectUuid)
    case 'app':
      return Boolean(s.applicationUuid)
    case 'deploy':
      return Boolean(s.deploymentUuid)
  }
}

// ---------------------------------------------------------------------------
// Steps
// ---------------------------------------------------------------------------

type Patch = (patch: Partial<OnboardingState>) => void

function WelcomeStep() {
  return (
    <div className="flex flex-col gap-3 text-sm text-zinc-400">
      <h2 className="text-lg font-semibold text-zinc-100">Welcome to rustify</h2>
      <p>This wizard connects your first server and ships your first application. You will:</p>
      <ol className="list-decimal space-y-1 pl-5">
        <li>Add an SSH private key</li>
        <li>Register a server and validate the connection</li>
        <li>Create a project and an application</li>
        <li>Trigger your first deployment</li>
      </ol>
    </div>
  )
}

function KeyStep({ state, patch }: { state: OnboardingState; patch: Patch }) {
  const [mode, setMode] = useState<'generate' | 'paste'>('generate')
  const [name, setName] = useState('default')
  const [privateKey, setPrivateKey] = useState('')

  const create = useMutation({
    mutationFn: () =>
      mode === 'generate'
        ? api.post<PrivateKey>('/private-keys/generate', { name })
        : api.post<PrivateKey>('/private-keys', { name, private_key: privateKey }),
    onSuccess: (key) => patch({ privateKeyUuid: key.uuid, publicKey: key.public_key }),
  })

  if (state.privateKeyUuid) {
    return (
      <div className="flex flex-col gap-3 text-sm">
        <p className="text-emerald-400">SSH key ready.</p>
        {state.publicKey && (
          <>
            <p className="text-zinc-400">
              Add this public key to <code className="text-zinc-200">~/.ssh/authorized_keys</code> on
              your server:
            </p>
            <pre className="overflow-x-auto rounded-md border border-zinc-800 bg-zinc-950 p-3 font-mono text-xs text-zinc-300">
              {state.publicKey}
            </pre>
          </>
        )}
      </div>
    )
  }

  return (
    <div className="flex max-w-xl flex-col gap-4">
      <div className="flex gap-2 text-sm">
        <button
          type="button"
          className={mode === 'generate' ? btnPrimary : btnGhost}
          onClick={() => setMode('generate')}
        >
          Generate new key
        </button>
        <button
          type="button"
          className={mode === 'paste' ? btnPrimary : btnGhost}
          onClick={() => setMode('paste')}
        >
          Paste existing key
        </button>
      </div>
      <Field label="Key name">
        <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      {mode === 'paste' && (
        <Field label="Private key (PEM)">
          <textarea
            className={`${inputCls} h-32 font-mono`}
            value={privateKey}
            onChange={(e) => setPrivateKey(e.target.value)}
            placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
          />
        </Field>
      )}
      <ErrorNote error={create.error} />
      <button
        type="button"
        className={`${btnPrimary} w-fit`}
        disabled={create.isPending || !name.trim() || (mode === 'paste' && !privateKey.trim())}
        onClick={() => create.mutate()}
      >
        {create.isPending ? 'Saving…' : mode === 'generate' ? 'Generate key' : 'Save key'}
      </button>
    </div>
  )
}

function ServerStep({ state, patch }: { state: OnboardingState; patch: Patch }) {
  const [name, setName] = useState('my-server')
  const [ip, setIp] = useState('')
  const [port, setPort] = useState('22')
  const [user, setUser] = useState('root')

  const create = useMutation({
    mutationFn: () =>
      api.post<Server>('/servers', {
        name,
        ip,
        port: Number(port) || 22,
        user,
        private_key_uuid: state.privateKeyUuid,
      }),
    onSuccess: (server) => patch({ serverUuid: server.uuid, serverName: server.name }),
  })

  if (state.serverUuid) {
    return (
      <p className="text-sm text-emerald-400">
        Server <span className="font-mono">{state.serverName}</span> registered.
      </p>
    )
  }

  return (
    <div className="flex max-w-xl flex-col gap-4">
      <Field label="Name">
        <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      <div className="grid grid-cols-3 gap-3">
        <div className="col-span-2">
          <Field label="IP address or hostname">
            <input className={inputCls} value={ip} onChange={(e) => setIp(e.target.value)} placeholder="203.0.113.1" />
          </Field>
        </div>
        <Field label="SSH port">
          <input className={inputCls} value={port} onChange={(e) => setPort(e.target.value)} />
        </Field>
      </div>
      <Field label="SSH user">
        <input className={inputCls} value={user} onChange={(e) => setUser(e.target.value)} />
      </Field>
      <ErrorNote error={create.error} />
      <button
        type="button"
        className={`${btnPrimary} w-fit`}
        disabled={create.isPending || !name.trim() || !ip.trim() || !user.trim()}
        onClick={() => create.mutate()}
      >
        {create.isPending ? 'Registering…' : 'Register server'}
      </button>
    </div>
  )
}

function ValidateStep({ state, patch }: { state: OnboardingState; patch: Patch }) {
  const [lines, setLines] = useState<string[]>([])
  const [running, setRunning] = useState(false)
  const serverUuid = state.serverUuid
  const outputRef = useRef<HTMLDivElement>(null)

  // Live validation output per C5: streams via WS channel `server:<uuid>`.
  useEffect(() => {
    if (!serverUuid) return
    return ws.subscribe(`server:${serverUuid}`, (env) => {
      if (env.event === 'server_reachability_changed') {
        const data = env.data as { reachable?: boolean; usable?: boolean } | null
        if (data?.usable) {
          patch({ serverValidated: true })
          setRunning(false)
        }
        setLines((prev) => [
          ...prev,
          `reachability changed: reachable=${String(data?.reachable ?? '?')} usable=${String(data?.usable ?? '?')}`,
        ])
        return
      }
      const data = env.data as Record<string, unknown> | string | null
      const text =
        typeof data === 'string'
          ? data
          : data && typeof data === 'object'
            ? String((data.content as string | undefined) ?? (data.line as string | undefined) ?? JSON.stringify(data))
            : env.event
      setLines((prev) => [...prev, text])
    })
    // patch is stable for the lifetime of the page
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serverUuid])

  useEffect(() => {
    const el = outputRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [lines])

  const validate = useMutation({
    mutationFn: () => api.post<{ job_uuid: string }>(`/servers/${serverUuid}/validate`),
    onSuccess: () => {
      setRunning(true)
      setLines((prev) => [...prev, 'validation started…'])
    },
  })

  const recheck = useMutation({
    mutationFn: () => api.get<Server>(`/servers/${serverUuid}`),
    onSuccess: (server) => {
      if (server.usable) {
        patch({ serverValidated: true })
        setRunning(false)
      }
      setLines((prev) => [
        ...prev,
        `server status: reachable=${String(server.reachable)} usable=${String(server.usable)}`,
      ])
    },
  })

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-2">
        <button
          type="button"
          className={btnPrimary}
          disabled={validate.isPending || running}
          onClick={() => validate.mutate()}
        >
          {running ? 'Validating…' : 'Run validation'}
        </button>
        <button type="button" className={btnGhost} disabled={recheck.isPending} onClick={() => recheck.mutate()}>
          Re-check status
        </button>
        {state.serverValidated && <span className="text-sm text-emerald-400">Server is usable.</span>}
      </div>
      <ErrorNote error={validate.error ?? recheck.error} />
      <div
        ref={outputRef}
        role="log"
        aria-label="validation output"
        className="h-64 overflow-auto rounded-lg border border-zinc-800 bg-zinc-950 p-3 font-mono text-xs text-zinc-300"
      >
        {lines.length === 0 ? (
          <span className="text-zinc-600">Validation output will stream here.</span>
        ) : (
          lines.map((l, i) => <div key={i}>{l}</div>)
        )}
      </div>
    </div>
  )
}

function ProjectStep({ state, patch }: { state: OnboardingState; patch: Patch }) {
  const [name, setName] = useState('my-project')
  const queryClient = useQueryClient()

  const create = useMutation({
    mutationFn: () => api.post<Project>('/projects', { name }),
    onSuccess: (project) => {
      queryClient.invalidateQueries({ queryKey: ['projects'] })
      patch({ projectUuid: project.uuid, projectName: project.name })
    },
  })

  if (state.projectUuid) {
    return (
      <p className="text-sm text-emerald-400">
        Project <span className="font-mono">{state.projectName}</span> created with a{' '}
        <span className="font-mono">production</span> environment.
      </p>
    )
  }

  return (
    <div className="flex max-w-xl flex-col gap-4">
      <Field label="Project name">
        <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      <ErrorNote error={create.error} />
      <button
        type="button"
        className={`${btnPrimary} w-fit`}
        disabled={create.isPending || !name.trim()}
        onClick={() => create.mutate()}
      >
        {create.isPending ? 'Creating…' : 'Create project'}
      </button>
    </div>
  )
}

const BUILD_PACKS: BuildPack[] = ['nixpacks', 'dockerfile', 'static', 'docker_image', 'docker_compose', 'railpack']

function AppStep({ state, patch }: { state: OnboardingState; patch: Patch }) {
  const [name, setName] = useState('my-app')
  const [repo, setRepo] = useState('')
  const [branch, setBranch] = useState('main')
  const [buildPack, setBuildPack] = useState<BuildPack>('nixpacks')
  const [ports, setPorts] = useState('80')
  const queryClient = useQueryClient()

  const create = useMutation({
    mutationFn: () =>
      api.post<Application>('/applications', {
        project_uuid: state.projectUuid,
        environment_name: 'production',
        server_uuid: state.serverUuid,
        name,
        git_repository: repo,
        git_branch: branch,
        build_pack: buildPack,
        ports_exposes: ports,
      }),
    onSuccess: (app) => {
      queryClient.invalidateQueries({ queryKey: ['applications'] })
      patch({ applicationUuid: app.uuid })
    },
  })

  if (state.applicationUuid) {
    return <p className="text-sm text-emerald-400">Application created.</p>
  }

  return (
    <div className="flex max-w-xl flex-col gap-4">
      <Field label="Application name">
        <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field label="Git repository">
        <input
          className={inputCls}
          value={repo}
          onChange={(e) => setRepo(e.target.value)}
          placeholder="https://github.com/acme/app.git"
        />
      </Field>
      <div className="grid grid-cols-3 gap-3">
        <Field label="Branch">
          <input className={inputCls} value={branch} onChange={(e) => setBranch(e.target.value)} />
        </Field>
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
        type="button"
        className={`${btnPrimary} w-fit`}
        disabled={create.isPending || !name.trim() || !repo.trim()}
        onClick={() => create.mutate()}
      >
        {create.isPending ? 'Creating…' : 'Create application'}
      </button>
    </div>
  )
}

function DeployStep({ state, patch }: { state: OnboardingState; patch: Patch }) {
  const deploy = useMutation({
    mutationFn: () =>
      api.post<{ deployment_uuid: string }>(`/applications/${state.applicationUuid}/deploy`, {}),
    onSuccess: (res) => patch({ deploymentUuid: res.deployment_uuid }),
  })

  if (state.deploymentUuid) {
    return (
      <p className="text-sm text-emerald-400">
        Deployment queued. Finish to watch the build logs live.
      </p>
    )
  }

  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-zinc-400">Everything is set up. Ship it.</p>
      <ErrorNote error={deploy.error} />
      <button
        type="button"
        className={`${btnPrimary} w-fit`}
        disabled={deploy.isPending}
        onClick={() => deploy.mutate()}
      >
        {deploy.isPending ? 'Queueing…' : 'Deploy now'}
      </button>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Route
// ---------------------------------------------------------------------------

export default function Onboarding() {
  const [state, setState] = useState<OnboardingState>({})
  const navigate = useNavigate()
  const patch: Patch = (p) => setState((prev) => ({ ...prev, ...p }))

  const contentFor = (id: OnboardingStepId) => {
    switch (id) {
      case 'welcome':
        return <WelcomeStep />
      case 'key':
        return <KeyStep state={state} patch={patch} />
      case 'server':
        return <ServerStep state={state} patch={patch} />
      case 'validate':
        return <ValidateStep state={state} patch={patch} />
      case 'project':
        return <ProjectStep state={state} patch={patch} />
      case 'app':
        return <AppStep state={state} patch={patch} />
      case 'deploy':
        return <DeployStep state={state} patch={patch} />
    }
  }

  const titles: Record<OnboardingStepId, string> = {
    welcome: 'Welcome',
    key: 'SSH key',
    server: 'Server',
    validate: 'Validate',
    project: 'Project',
    app: 'Application',
    deploy: 'Deploy',
  }

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-6">
      <PageTitle>Onboarding</PageTitle>
      <div className={cardCls}>
        <Wizard
          finishLabel="Watch deployment"
          onFinish={() => {
            if (state.deploymentUuid) navigate(`/deployments/${state.deploymentUuid}`)
          }}
          steps={ONBOARDING_STEPS.map((id) => ({
            id,
            title: titles[id],
            canAdvance: canLeaveStep(id, state),
            content: contentFor(id),
          }))}
        />
      </div>
    </div>
  )
}
