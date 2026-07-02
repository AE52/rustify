import { useMutation, useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from 'react-router'
import { api, type Deployment } from '../../../api/client'
import { useApplication } from './index'
import { StatusBadge } from '../../../components/StatusBadge'
import { btnGhost, ErrorNote, SectionTitle } from '../../../components/ui'

function when(ts: string | null | undefined): string {
  if (!ts) return '—'
  return new Date(ts).toLocaleString()
}

export default function ApplicationDeployments() {
  const { app } = useApplication()
  const navigate = useNavigate()

  const deployments = useQuery({
    queryKey: ['deployments', app.uuid],
    queryFn: () => api.get<Deployment[]>(`/deployments?application_uuid=${app.uuid}`),
    refetchInterval: 10_000,
  })

  const deploy = useMutation({
    mutationFn: (forceRebuild: boolean) =>
      api.post<{ deployment_uuid: string }>(`/applications/${app.uuid}/deploy`, {
        force_rebuild: forceRebuild,
      }),
    onSuccess: (res) => navigate(`/deployments/${res.deployment_uuid}`),
  })

  return (
    <div className="flex max-w-3xl flex-col gap-4">
      <div className="flex items-center justify-between">
        <SectionTitle>Deployments</SectionTitle>
        <button
          type="button"
          className={btnGhost}
          disabled={deploy.isPending}
          onClick={() => deploy.mutate(true)}
          title="Deploy ignoring build cache"
        >
          Force rebuild
        </button>
      </div>
      <ErrorNote error={deployments.error ?? deploy.error} />

      <div className="flex flex-col gap-2">
        {deployments.data?.map((d) => (
          <Link
            key={d.uuid}
            to={`/deployments/${d.uuid}`}
            className="flex items-center gap-4 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-2.5 hover:border-zinc-600"
          >
            <StatusBadge status={d.status} />
            <span className="font-mono text-xs text-zinc-400">
              {d.commit_sha ? d.commit_sha.slice(0, 8) : 'HEAD'}
            </span>
            <span className="min-w-0 truncate text-sm text-zinc-300">{d.commit_message ?? ''}</span>
            <span className="ml-auto shrink-0 text-xs text-zinc-500">{when(d.created_at)}</span>
            {d.force_rebuild && (
              <span className="shrink-0 rounded-full border border-zinc-700 px-2 py-0.5 text-xs text-zinc-500">
                force
              </span>
            )}
          </Link>
        ))}
        {deployments.data?.length === 0 && (
          <p className="text-sm text-zinc-500">No deployments yet. Hit Deploy to ship the first one.</p>
        )}
      </div>
    </div>
  )
}
