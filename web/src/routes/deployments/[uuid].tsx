import { useCallback, useEffect, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useParams } from 'react-router'
import { api, type DeploymentDetail, type LogLine } from '../../api/client'
import { ws } from '../../api/ws'
import { LogViewer } from '../../components/LogViewer'
import { StatusBadge } from '../../components/StatusBadge'
import { btnGhost, ErrorNote, PageTitle } from '../../components/ui'

function extractLogLine(data: unknown): LogLine | null {
  if (!data || typeof data !== 'object') return null
  const maybe = 'line' in data ? (data as { line: unknown }).line : data
  if (maybe && typeof maybe === 'object' && typeof (maybe as LogLine).order === 'number') {
    return maybe as LogLine
  }
  return null
}

export default function DeploymentPage() {
  const { uuid = '' } = useParams()
  const queryClient = useQueryClient()
  const [refreshKey, setRefreshKey] = useState(0)

  const deployment = useQuery({
    queryKey: ['deployment', uuid],
    queryFn: () => api.get<DeploymentDetail>(`/deployments/${uuid}`),
  })

  // Status changes stream on the deployment channel (C4).
  useEffect(() => {
    const offChannel = ws.subscribe(`deployment:${uuid}`, (env) => {
      if (env.event === 'deployment_status_changed') {
        queryClient.invalidateQueries({ queryKey: ['deployment', uuid] })
      }
    })
    // after a WS reconnect we may have missed lines: force a refetch (deduped by ord)
    const offOpen = ws.onOpen(() => setRefreshKey((k) => k + 1))
    return () => {
      offChannel()
      offOpen()
    }
  }, [uuid, queryClient])

  const fetchLines = useCallback(
    async () => (await api.get<DeploymentDetail>(`/deployments/${uuid}`)).logs,
    [uuid],
  )

  const subscribeLines = useCallback(
    (onLine: (line: LogLine) => void) =>
      ws.subscribe(`deployment:${uuid}`, (env) => {
        if (env.event !== 'deployment_log_appended') return
        const line = extractLogLine(env.data)
        if (line) onLine(line)
      }),
    [uuid],
  )

  const cancel = useMutation({
    mutationFn: () => api.post(`/deployments/${uuid}/cancel`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['deployment', uuid] }),
  })

  if (deployment.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (deployment.isError) return <ErrorNote error={deployment.error} />

  const d = deployment.data
  const cancellable = d.status === 'queued' || d.status === 'in_progress'

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-center gap-4">
        <PageTitle>Deployment</PageTitle>
        <StatusBadge status={d.status} />
        <span className="font-mono text-xs text-zinc-500">
          {d.commit_sha ? d.commit_sha.slice(0, 8) : 'HEAD'}
        </span>
        {d.commit_message && (
          <span className="min-w-0 truncate text-sm text-zinc-400">{d.commit_message}</span>
        )}
        <div className="ml-auto flex items-center gap-2">
          <Link to={`/applications/${d.application_uuid}`} className={btnGhost}>
            Application
          </Link>
          {cancellable && (
            <button
              type="button"
              className={btnGhost}
              disabled={cancel.isPending}
              onClick={() => cancel.mutate()}
            >
              {cancel.isPending ? 'Cancelling…' : 'Cancel'}
            </button>
          )}
        </div>
      </div>
      <ErrorNote error={cancel.error} />

      <dl className="flex flex-wrap gap-x-8 gap-y-1 text-xs text-zinc-500">
        <div>
          <dt className="inline">created: </dt>
          <dd className="inline text-zinc-400">{new Date(d.created_at).toLocaleString()}</dd>
        </div>
        <div>
          <dt className="inline">started: </dt>
          <dd className="inline text-zinc-400">
            {d.started_at ? new Date(d.started_at).toLocaleString() : '—'}
          </dd>
        </div>
        <div>
          <dt className="inline">finished: </dt>
          <dd className="inline text-zinc-400">
            {d.finished_at ? new Date(d.finished_at).toLocaleString() : '—'}
          </dd>
        </div>
      </dl>

      <LogViewer fetchLines={fetchLines} subscribe={subscribeLines} refreshKey={refreshKey} height={560} />
    </div>
  )
}
