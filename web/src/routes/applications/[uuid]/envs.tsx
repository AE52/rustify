import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { api, type EnvVar } from '../../../api/client'
import { useApplication } from './index'
import {
  btnDanger,
  btnGhost,
  btnPrimary,
  cardCls,
  ErrorNote,
  Field,
  inputCls,
  SectionTitle,
} from '../../../components/ui'

function EnvRow({ appUuid, env }: { appUuid: string; env: EnvVar }) {
  const queryClient = useQueryClient()
  const [editing, setEditing] = useState(false)
  const [value, setValue] = useState('')

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ['application', appUuid, 'envs'] })

  const update = useMutation({
    mutationFn: () => api.patch<EnvVar>(`/applications/${appUuid}/envs/${env.uuid}`, { value }),
    onSuccess: () => {
      setEditing(false)
      invalidate()
    },
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/applications/${appUuid}/envs/${env.uuid}`),
    onSuccess: invalidate,
  })

  const flags = [
    env.is_buildtime ? 'build' : null,
    env.is_literal ? 'literal' : null,
    env.is_shown_once ? 'shown once' : null,
  ].filter(Boolean)

  return (
    <div className="flex flex-col gap-2 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-2.5">
      <div className="flex items-center gap-3">
        <span className="font-mono text-sm text-zinc-100">{env.key}</span>
        {flags.length > 0 && <span className="text-xs text-zinc-500">{flags.join(' · ')}</span>}
        <div className="ml-auto flex gap-2">
          <button
            type="button"
            className={`${btnGhost} py-1 text-xs`}
            onClick={() => {
              setEditing((v) => !v)
              setValue(env.value ?? '')
            }}
          >
            {editing ? 'Cancel' : 'Edit'}
          </button>
          <button
            type="button"
            className={`${btnDanger} py-1 text-xs`}
            disabled={remove.isPending}
            onClick={() => remove.mutate()}
          >
            Delete
          </button>
        </div>
      </div>
      {!editing && (
        <code className="truncate font-mono text-xs text-zinc-400">
          {env.value ?? (env.is_shown_once ? '(hidden — shown once at creation)' : '(empty)')}
        </code>
      )}
      {editing && (
        <form
          onSubmit={(e: FormEvent) => {
            e.preventDefault()
            update.mutate()
          }}
          className="flex gap-2"
        >
          <input
            aria-label={`value for ${env.key}`}
            className={`${inputCls} font-mono`}
            value={value}
            onChange={(e) => setValue(e.target.value)}
          />
          <button type="submit" className={btnPrimary} disabled={update.isPending}>
            Save
          </button>
        </form>
      )}
      <ErrorNote error={update.error ?? remove.error} />
    </div>
  )
}

export default function ApplicationEnvs() {
  const { app } = useApplication()
  const queryClient = useQueryClient()
  const [key, setKey] = useState('')
  const [value, setValue] = useState('')
  const [isBuildtime, setIsBuildtime] = useState(false)
  const [isLiteral, setIsLiteral] = useState(false)
  const [isShownOnce, setIsShownOnce] = useState(false)

  const envs = useQuery({
    queryKey: ['application', app.uuid, 'envs'],
    queryFn: () => api.get<EnvVar[]>(`/applications/${app.uuid}/envs`),
  })

  const create = useMutation({
    mutationFn: () =>
      api.post<EnvVar>(`/applications/${app.uuid}/envs`, {
        key,
        value,
        is_buildtime: isBuildtime,
        is_literal: isLiteral,
        is_shown_once: isShownOnce,
      }),
    onSuccess: () => {
      setKey('')
      setValue('')
      setIsBuildtime(false)
      setIsLiteral(false)
      setIsShownOnce(false)
      queryClient.invalidateQueries({ queryKey: ['application', app.uuid, 'envs'] })
    },
  })

  const submit = (e: FormEvent) => {
    e.preventDefault()
    if (key.trim()) create.mutate()
  }

  return (
    <div className="flex flex-col gap-6">
      <form onSubmit={submit} className={`${cardCls} flex max-w-3xl flex-col gap-4`}>
        <SectionTitle>Add environment variable</SectionTitle>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Key">
            <input
              className={`${inputCls} font-mono`}
              value={key}
              onChange={(e) => setKey(e.target.value)}
              placeholder="DATABASE_URL"
            />
          </Field>
          <Field label="Value">
            <input className={`${inputCls} font-mono`} value={value} onChange={(e) => setValue(e.target.value)} />
          </Field>
        </div>
        <div className="flex flex-wrap gap-4 text-sm text-zinc-300">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={isBuildtime}
              onChange={(e) => setIsBuildtime(e.target.checked)}
              className="accent-zinc-400"
            />
            Available at build time
          </label>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={isLiteral}
              onChange={(e) => setIsLiteral(e.target.checked)}
              className="accent-zinc-400"
            />
            Literal (no interpolation)
          </label>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={isShownOnce}
              onChange={(e) => setIsShownOnce(e.target.checked)}
              className="accent-zinc-400"
            />
            Show value only once
          </label>
        </div>
        <ErrorNote error={create.error} />
        <button type="submit" className={`${btnPrimary} w-fit`} disabled={!key.trim() || create.isPending}>
          {create.isPending ? 'Adding…' : 'Add variable'}
        </button>
      </form>

      <section className="flex max-w-3xl flex-col gap-2">
        <SectionTitle>Variables</SectionTitle>
        <ErrorNote error={envs.error} />
        {envs.data?.map((env) => <EnvRow key={env.uuid} appUuid={app.uuid} env={env} />)}
        {envs.data?.length === 0 && <p className="text-sm text-zinc-500">No environment variables.</p>}
      </section>
    </div>
  )
}
