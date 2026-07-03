import { useState, type FormEvent } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import { useNavigate } from 'react-router'
import {
  api,
  type Database,
  type Environment,
  type Project,
  type Server,
} from '../../api/client'
import { DATABASE_ENGINES } from '../../lib/engines'
import {
  btnPrimary,
  cardCls,
  ErrorNote,
  Field,
  inputCls,
  PageTitle,
  SectionTitle,
  selectCls,
} from '../../components/ui'

/** "New Resource" — create a standalone database in a project/environment. */
export default function NewDatabase() {
  const navigate = useNavigate()
  const [projectUuid, setProjectUuid] = useState('')
  const [environmentName, setEnvironmentName] = useState('')
  const [serverUuid, setServerUuid] = useState('')
  const [engine, setEngine] = useState('postgresql')
  const [name, setName] = useState('')
  const [image, setImage] = useState('')
  const [isPublic, setIsPublic] = useState(false)
  const [publicPort, setPublicPort] = useState('')

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

  const create = useMutation({
    mutationFn: () =>
      api.post<Database>('/databases', {
        project_uuid: selectedProject,
        environment_name: selectedEnv,
        server_uuid: selectedServer,
        engine,
        name,
        image: image.trim() || null,
        is_public: isPublic,
        public_port: isPublic && publicPort ? Number(publicPort) : null,
      }),
    onSuccess: (db) => navigate(`/databases/${db.uuid}`),
  })

  const canSubmit =
    name.trim() !== '' && Boolean(selectedProject) && Boolean(selectedEnv) && Boolean(selectedServer) && !create.isPending

  const submit = (e: FormEvent) => {
    e.preventDefault()
    if (canSubmit) create.mutate()
  }

  return (
    <div className="flex flex-col gap-6">
      <PageTitle>New database</PageTitle>
      <form onSubmit={submit} className={`${cardCls} flex max-w-2xl flex-col gap-4`}>
        <SectionTitle>Resource</SectionTitle>
        <Field label="Engine">
          <select className={selectCls} value={engine} onChange={(e) => setEngine(e.target.value)}>
            {DATABASE_ENGINES.map((eng) => (
              <option key={eng.value} value={eng.value}>
                {eng.label}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Name">
          <input
            className={inputCls}
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="my-database"
          />
        </Field>
        <Field label="Image (optional — defaults to engine image)">
          <input
            className={`${inputCls} font-mono`}
            value={image}
            onChange={(e) => setImage(e.target.value)}
            placeholder="postgres:16-alpine"
          />
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
        <div className="flex items-end gap-3">
          <label className="flex items-center gap-2 pb-1.5 text-sm text-zinc-300">
            <input
              type="checkbox"
              checked={isPublic}
              onChange={(e) => setIsPublic(e.target.checked)}
              className="accent-zinc-400"
            />
            Make publicly accessible
          </label>
          <div className="flex-1">
            <Field label="Public port">
              <input
                className={inputCls}
                value={publicPort}
                onChange={(e) => setPublicPort(e.target.value)}
                disabled={!isPublic}
                inputMode="numeric"
                placeholder="5432"
              />
            </Field>
          </div>
        </div>
        <ErrorNote error={create.error ?? projects.error ?? servers.error} />
        <button type="submit" className={`${btnPrimary} w-fit`} disabled={!canSubmit}>
          {create.isPending ? 'Creating…' : 'Create database'}
        </button>
      </form>
    </div>
  )
}
