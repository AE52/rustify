import { useEffect, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate, useParams } from 'react-router'
import {
  api,
  type Backup,
  type BackupExecution,
  type Database,
  type S3Storage,
} from '../../api/client'
import { ws } from '../../api/ws'
import { connectionStrings, engineLabel } from '../../lib/engines'
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

type Tab = 'general' | 'backups' | 'danger'

// ----- General ------------------------------------------------------------

function GeneralTab({ db, refetch }: { db: Database; refetch: () => void }) {
  const [image, setImage] = useState(db.image)
  const [isPublic, setIsPublic] = useState(db.is_public)
  const [publicPort, setPublicPort] = useState(db.public_port ? String(db.public_port) : '')

  const save = useMutation({
    mutationFn: () =>
      api.patch<Database>(`/databases/${db.uuid}`, {
        image,
        is_public: isPublic,
        public_port: isPublic && publicPort ? Number(publicPort) : null,
      }),
    onSuccess: () => refetch(),
  })

  const strings = connectionStrings(db)

  return (
    <div className="flex flex-col gap-8">
      <form
        onSubmit={(e: FormEvent) => {
          e.preventDefault()
          save.mutate()
        }}
        className={`${cardCls} flex max-w-2xl flex-col gap-4`}
      >
        <SectionTitle>General</SectionTitle>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Engine">
            <input className={inputCls} value={engineLabel(db.engine)} disabled />
          </Field>
          <Field label="Image">
            <input
              className={`${inputCls} font-mono`}
              value={image}
              onChange={(e) => setImage(e.target.value)}
            />
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
            Publicly accessible
          </label>
          <div className="flex-1">
            <Field label="Public port">
              <input
                className={inputCls}
                value={publicPort}
                onChange={(e) => setPublicPort(e.target.value)}
                disabled={!isPublic}
                inputMode="numeric"
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
        <SectionTitle>Connection strings</SectionTitle>
        <p className="mb-2 text-xs text-zinc-500">
          Credentials are managed server-side; the user and password segments are shown as
          placeholders.
        </p>
        <div className="flex flex-col gap-2">
          {strings.map((s) => (
            <div key={s.label} className="flex items-center gap-3">
              <span className="w-16 shrink-0 text-xs text-zinc-500">{s.label}</span>
              <code className="min-w-0 flex-1 truncate rounded bg-zinc-950 px-2 py-1.5 font-mono text-xs text-zinc-300">
                {s.value}
              </code>
            </div>
          ))}
        </div>
      </section>
    </div>
  )
}

// ----- Backups ------------------------------------------------------------

function fmtSize(bytes: number): string {
  if (!bytes) return '—'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let n = bytes
  let i = 0
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024
    i += 1
  }
  return `${n.toFixed(i === 0 ? 0 : 1)} ${units[i]}`
}

function BackupExecutions({ backupUuid }: { backupUuid: string }) {
  const queryClient = useQueryClient()
  const execs = useQuery({
    queryKey: ['backup', backupUuid, 'executions'],
    queryFn: () => api.get<BackupExecution[]>(`/backups/${backupUuid}/executions`),
  })

  useEffect(() => {
    return ws.subscribe(`backup:${backupUuid}`, (env) => {
      if (env.event === 'backup_status_changed') {
        queryClient.invalidateQueries({ queryKey: ['backup', backupUuid, 'executions'] })
      }
    })
  }, [backupUuid, queryClient])

  if (execs.isPending) return <p className="text-xs text-zinc-500">Loading executions…</p>
  if (execs.isError) return <ErrorNote error={execs.error} />
  if (execs.data.length === 0) return <p className="text-xs text-zinc-500">No executions yet.</p>

  return (
    <div className="flex flex-col gap-1">
      {execs.data.map((e) => (
        <div
          key={e.uuid}
          className="flex items-center gap-3 rounded-md border border-zinc-800 bg-zinc-950/40 px-3 py-1.5 text-xs"
        >
          <StatusBadge status={e.status} />
          <span className="text-zinc-500">{new Date(e.started_at).toLocaleString()}</span>
          <span className="text-zinc-500">{fmtSize(e.size)}</span>
          {e.s3_uploaded && <span className="text-emerald-400">S3</span>}
          {e.filename && (
            <span className="ml-auto min-w-0 truncate font-mono text-zinc-400" title={e.filename}>
              {e.filename}
            </span>
          )}
          {e.message && !e.filename && (
            <span className="ml-auto min-w-0 truncate text-zinc-400">{e.message}</span>
          )}
        </div>
      ))}
    </div>
  )
}

function BackupRow({ backup }: { backup: Backup }) {
  const queryClient = useQueryClient()
  const [open, setOpen] = useState(false)

  const trigger = useMutation({
    mutationFn: () => api.post(`/backups/${backup.uuid}/trigger`),
    onSuccess: () => {
      setOpen(true)
      queryClient.invalidateQueries({ queryKey: ['backup', backup.uuid, 'executions'] })
    },
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/backups/${backup.uuid}`),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ['database', backup.database_uuid, 'backups'] }),
  })

  return (
    <div className="flex flex-col gap-2 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-3">
      <div className="flex items-center gap-3">
        <code className="font-mono text-sm text-zinc-100">{backup.frequency}</code>
        {!backup.enabled && <span className="text-xs text-amber-400">disabled</span>}
        {backup.save_s3 && <span className="text-xs text-sky-400">→ S3</span>}
        <span className="text-xs text-zinc-500">
          keep {backup.retention_amount_local} / {backup.retention_days_local}d
        </span>
        <div className="ml-auto flex gap-2">
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
      <ErrorNote error={trigger.error ?? remove.error} />
      {open && <BackupExecutions backupUuid={backup.uuid} />}
    </div>
  )
}

function BackupsTab({ db }: { db: Database }) {
  const queryClient = useQueryClient()
  const [frequency, setFrequency] = useState('daily')
  const [retentionAmount, setRetentionAmount] = useState('7')
  const [retentionDays, setRetentionDays] = useState('30')
  const [saveS3, setSaveS3] = useState(false)
  const [s3Uuid, setS3Uuid] = useState('')

  const backups = useQuery({
    queryKey: ['database', db.uuid, 'backups'],
    queryFn: () => api.get<Backup[]>(`/databases/${db.uuid}/backups`),
  })
  const s3s = useQuery({
    queryKey: ['s3-storages'],
    queryFn: () => api.get<S3Storage[]>('/s3-storages'),
  })

  const create = useMutation({
    mutationFn: () =>
      api.post<Backup>(`/databases/${db.uuid}/backups`, {
        frequency,
        enabled: true,
        save_s3: saveS3,
        s3_storage_uuid: saveS3 && s3Uuid ? s3Uuid : null,
        retention_amount_local: Number(retentionAmount) || 0,
        retention_days_local: Number(retentionDays) || 0,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['database', db.uuid, 'backups'] })
    },
  })

  const canSubmit = frequency.trim() !== '' && (!saveS3 || Boolean(s3Uuid)) && !create.isPending

  return (
    <div className="flex flex-col gap-6">
      <form
        onSubmit={(e: FormEvent) => {
          e.preventDefault()
          if (canSubmit) create.mutate()
        }}
        className={`${cardCls} flex max-w-2xl flex-col gap-4`}
      >
        <SectionTitle>New scheduled backup</SectionTitle>
        <div className="grid grid-cols-3 gap-3">
          <Field label="Frequency (cron or alias)">
            <input
              className={`${inputCls} font-mono`}
              value={frequency}
              onChange={(e) => setFrequency(e.target.value)}
              placeholder="daily"
            />
          </Field>
          <Field label="Keep last (count)">
            <input
              className={inputCls}
              value={retentionAmount}
              onChange={(e) => setRetentionAmount(e.target.value)}
              inputMode="numeric"
            />
          </Field>
          <Field label="Keep days">
            <input
              className={inputCls}
              value={retentionDays}
              onChange={(e) => setRetentionDays(e.target.value)}
              inputMode="numeric"
            />
          </Field>
        </div>
        <div className="flex items-end gap-3">
          <label className="flex items-center gap-2 pb-1.5 text-sm text-zinc-300">
            <input
              type="checkbox"
              checked={saveS3}
              onChange={(e) => setSaveS3(e.target.checked)}
              className="accent-zinc-400"
            />
            Upload to S3
          </label>
          <div className="flex-1">
            <Field label="S3 storage">
              <select
                className={selectCls}
                value={s3Uuid}
                onChange={(e) => setS3Uuid(e.target.value)}
                disabled={!saveS3}
              >
                <option value="">Select storage…</option>
                {s3s.data?.map((s) => (
                  <option key={s.uuid} value={s.uuid}>
                    {s.name}
                  </option>
                ))}
              </select>
            </Field>
          </div>
        </div>
        <ErrorNote error={create.error} />
        <button type="submit" className={`${btnPrimary} w-fit`} disabled={!canSubmit}>
          {create.isPending ? 'Creating…' : 'Create backup'}
        </button>
      </form>

      <section className="flex max-w-2xl flex-col gap-2">
        <SectionTitle>Schedules</SectionTitle>
        <ErrorNote error={backups.error} />
        {backups.data?.map((b) => <BackupRow key={b.uuid} backup={b} />)}
        {backups.data?.length === 0 && <p className="text-sm text-zinc-500">No backup schedules.</p>}
      </section>
    </div>
  )
}

// ----- Page ---------------------------------------------------------------

export default function DatabasePage() {
  const { uuid = '' } = useParams()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [tab, setTab] = useState<Tab>('general')

  const database = useQuery({
    queryKey: ['database', uuid],
    queryFn: () => api.get<Database>(`/databases/${uuid}`),
    refetchInterval: 15_000,
  })

  // Live container status on `database:<uuid>` (C4).
  useEffect(() => {
    return ws.subscribe(`database:${uuid}`, (env) => {
      if (env.event === 'database_status_changed') {
        queryClient.invalidateQueries({ queryKey: ['database', uuid] })
      }
    })
  }, [uuid, queryClient])

  const lifecycle = useMutation({
    mutationFn: (action: 'start' | 'stop' | 'restart') =>
      api.post(`/databases/${uuid}/${action}`),
    onSuccess: () => database.refetch(),
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/databases/${uuid}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['databases'] })
      navigate('/databases')
    },
  })

  if (database.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (database.isError) return <ErrorNote error={database.error} />

  const d = database.data

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-center gap-4">
        <PageTitle>{d.name}</PageTitle>
        <StatusBadge status={d.status} />
        <span className="text-xs text-zinc-500">{engineLabel(d.engine)}</span>
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
            onClick={() => lifecycle.mutate('start')}
          >
            {lifecycle.isPending ? 'Working…' : 'Start'}
          </button>
        </div>
      </div>
      <ErrorNote error={lifecycle.error} />

      <nav className="flex gap-1 border-b border-zinc-800 text-sm">
        {(['general', 'backups', 'danger'] as const).map((t) => (
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
      </nav>

      {tab === 'general' && <GeneralTab db={d} refetch={() => database.refetch()} />}
      {tab === 'backups' && <BackupsTab db={d} />}
      {tab === 'danger' && (
        <section className="max-w-2xl">
          <SectionTitle>Danger zone</SectionTitle>
          <ConfirmDanger
            label="Delete database"
            confirmText={d.name}
            description="Deletes this database, its schedules and backup history."
            busy={remove.isPending}
            onConfirm={() => remove.mutate()}
          />
          <ErrorNote error={remove.error} />
        </section>
      )}
    </div>
  )
}
