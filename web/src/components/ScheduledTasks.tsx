import { useEffect, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  api,
  type ScheduledTask,
  type ScheduledTaskExecution,
} from '../api/client'
import { ws } from '../api/ws'
import { CRON_ALIASES, isValidFrequency } from '../lib/cron'
import { StatusBadge } from './StatusBadge'
import {
  btnGhost,
  btnPrimary,
  cardCls,
  ErrorNote,
  Field,
  inputCls,
  SectionTitle,
  selectCls,
} from './ui'

/** Which parent a task hangs off; drives the create/list endpoints. */
export type TaskResource = 'applications' | 'services'

function TaskExecutions({ taskUuid }: { taskUuid: string }) {
  const queryClient = useQueryClient()
  const execs = useQuery({
    queryKey: ['scheduled-task', taskUuid, 'executions'],
    queryFn: () =>
      api.get<ScheduledTaskExecution[]>(`/scheduled-tasks/${taskUuid}/executions`),
  })

  // Executions change state on `scheduled-task:<uuid>` (C4).
  useEffect(() => {
    return ws.subscribe(`scheduled-task:${taskUuid}`, (env) => {
      if (env.event === 'scheduled_task_status_changed') {
        queryClient.invalidateQueries({
          queryKey: ['scheduled-task', taskUuid, 'executions'],
        })
      }
    })
  }, [taskUuid, queryClient])

  if (execs.isPending) return <p className="text-xs text-zinc-500">Loading executions…</p>
  if (execs.isError) return <ErrorNote error={execs.error} />
  if (execs.data.length === 0)
    return <p className="text-xs text-zinc-500">No executions yet.</p>

  return (
    <div className="flex flex-col gap-1">
      {execs.data.map((e) => (
        <div
          key={e.uuid}
          className="flex items-center gap-3 rounded-md border border-zinc-800 bg-zinc-950/40 px-3 py-1.5 text-xs"
        >
          <StatusBadge status={e.status} />
          <span className="text-zinc-500">{new Date(e.started_at).toLocaleString()}</span>
          {typeof e.duration === 'number' && (
            <span className="text-zinc-500">{e.duration}s</span>
          )}
          {e.message && <span className="min-w-0 truncate text-zinc-400">{e.message}</span>}
        </div>
      ))}
    </div>
  )
}

function TaskRow({ task }: { task: ScheduledTask }) {
  const queryClient = useQueryClient()
  const [open, setOpen] = useState(false)

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ['scheduled-task', task.uuid] })

  const toggle = useMutation({
    mutationFn: () =>
      api.patch<ScheduledTask>(`/scheduled-tasks/${task.uuid}`, { enabled: !task.enabled }),
    onSuccess: invalidate,
  })

  const trigger = useMutation({
    mutationFn: () => api.post(`/scheduled-tasks/${task.uuid}/trigger`),
    onSuccess: () =>
      queryClient.invalidateQueries({
        queryKey: ['scheduled-task', task.uuid, 'executions'],
      }),
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/scheduled-tasks/${task.uuid}`),
    onSuccess: invalidate,
  })

  return (
    <div className="flex flex-col gap-2 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-3">
      <div className="flex items-center gap-3">
        <span className="font-medium text-zinc-100">{task.name}</span>
        <code className="font-mono text-xs text-zinc-500">{task.frequency}</code>
        {!task.enabled && <span className="text-xs text-amber-400">disabled</span>}
        <div className="ml-auto flex gap-2">
          <label className="flex items-center gap-1.5 text-xs text-zinc-400">
            <input
              type="checkbox"
              checked={task.enabled}
              onChange={() => toggle.mutate()}
              disabled={toggle.isPending}
              className="accent-zinc-400"
              aria-label={`enable ${task.name}`}
            />
            Enabled
          </label>
          <button
            type="button"
            className={`${btnGhost} py-1 text-xs`}
            disabled={trigger.isPending}
            onClick={() => trigger.mutate()}
          >
            {trigger.isPending ? 'Triggering…' : 'Trigger now'}
          </button>
          <button
            type="button"
            className={`${btnGhost} py-1 text-xs`}
            onClick={() => setOpen((v) => !v)}
          >
            {open ? 'Hide runs' : 'Runs'}
          </button>
          <button
            type="button"
            className={`${btnGhost} py-1 text-xs`}
            disabled={remove.isPending}
            onClick={() => remove.mutate()}
          >
            Delete
          </button>
        </div>
      </div>
      <code className="truncate font-mono text-xs text-zinc-400">
        {task.command}
        {task.container ? `  @ ${task.container}` : ''}
      </code>
      <ErrorNote error={toggle.error ?? trigger.error ?? remove.error} />
      {open && <TaskExecutions taskUuid={task.uuid} />}
    </div>
  )
}

export function ScheduledTasks({ resource, uuid }: { resource: TaskResource; uuid: string }) {
  const queryClient = useQueryClient()
  const [name, setName] = useState('')
  const [command, setCommand] = useState('')
  const [frequency, setFrequency] = useState('daily')
  const [container, setContainer] = useState('')
  const [timeout, setTimeout] = useState('300')

  const tasks = useQuery({
    queryKey: ['scheduled-tasks', resource, uuid],
    queryFn: () => api.get<ScheduledTask[]>(`/${resource}/${uuid}/scheduled-tasks`),
  })

  const create = useMutation({
    mutationFn: () =>
      api.post<ScheduledTask>(`/${resource}/${uuid}/scheduled-tasks`, {
        name,
        command,
        frequency,
        container: container.trim() || null,
        timeout: Number(timeout) || null,
      }),
    onSuccess: () => {
      setName('')
      setCommand('')
      setFrequency('daily')
      setContainer('')
      setTimeout('300')
      queryClient.invalidateQueries({ queryKey: ['scheduled-tasks', resource, uuid] })
    },
  })

  const freqValid = isValidFrequency(frequency)
  const canSubmit = name.trim() !== '' && command.trim() !== '' && freqValid && !create.isPending

  const submit = (e: FormEvent) => {
    e.preventDefault()
    if (canSubmit) create.mutate()
  }

  return (
    <div className="flex flex-col gap-6">
      <form onSubmit={submit} className={`${cardCls} flex max-w-3xl flex-col gap-4`}>
        <SectionTitle>New scheduled task</SectionTitle>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Name">
            <input
              className={inputCls}
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="db-cleanup"
            />
          </Field>
          <Field label="Container (optional)">
            <input
              className={inputCls}
              value={container}
              onChange={(e) => setContainer(e.target.value)}
              placeholder="defaults to the main container"
            />
          </Field>
        </div>
        <Field label="Command">
          <input
            className={`${inputCls} font-mono`}
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            placeholder="php artisan schedule:run"
          />
        </Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Frequency (cron or alias)">
            <input
              className={`${inputCls} font-mono`}
              value={frequency}
              onChange={(e) => setFrequency(e.target.value)}
              placeholder="daily or 0 3 * * *"
              aria-invalid={!freqValid}
            />
          </Field>
          <Field label="Timeout (seconds)">
            <input
              className={inputCls}
              value={timeout}
              onChange={(e) => setTimeout(e.target.value)}
              inputMode="numeric"
            />
          </Field>
        </div>
        <div className="flex flex-wrap gap-1.5">
          {CRON_ALIASES.map((a) => (
            <button
              key={a.value}
              type="button"
              onClick={() => setFrequency(a.value)}
              className={`${selectCls} w-auto px-2 py-1 text-xs`}
            >
              {a.label}
            </button>
          ))}
        </div>
        {!freqValid && frequency.trim() !== '' && (
          <p className="text-xs text-red-400">Not a valid cron expression or alias.</p>
        )}
        <ErrorNote error={create.error} />
        <button type="submit" className={`${btnPrimary} w-fit`} disabled={!canSubmit}>
          {create.isPending ? 'Creating…' : 'Create task'}
        </button>
      </form>

      <section className="flex max-w-3xl flex-col gap-2">
        <SectionTitle>Tasks</SectionTitle>
        <ErrorNote error={tasks.error} />
        {tasks.data?.map((t) => <TaskRow key={t.uuid} task={t} />)}
        {tasks.data?.length === 0 && <p className="text-sm text-zinc-500">No scheduled tasks.</p>}
      </section>
    </div>
  )
}
