import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { api, type Preview, type PreviewRedeployResponse } from '../../../api/client'
import { useApplication } from './index'
import { StatusBadge } from '../../../components/StatusBadge'
import { btnDanger, btnGhost, ErrorNote, SectionTitle } from '../../../components/ui'

function PreviewRow({ appUuid, preview }: { appUuid: string; preview: Preview }) {
  const queryClient = useQueryClient()
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ['application-previews', appUuid] })

  const redeploy = useMutation({
    mutationFn: () =>
      api.post<PreviewRedeployResponse>(
        `/applications/${appUuid}/previews/${preview.pull_request_id}/redeploy`,
      ),
    onSuccess: invalidate,
  })

  const cleanup = useMutation({
    mutationFn: () => api.delete(`/applications/${appUuid}/previews/${preview.pull_request_id}`),
    onSuccess: invalidate,
  })

  return (
    <div className="flex items-center gap-4 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-2.5">
      <StatusBadge status={preview.status} />
      <span className="font-mono text-xs text-zinc-300">PR #{preview.pull_request_id}</span>
      {preview.pull_request_html_url && (
        <a
          href={preview.pull_request_html_url}
          target="_blank"
          rel="noreferrer"
          className="text-xs text-zinc-400 underline-offset-2 hover:text-zinc-200 hover:underline"
        >
          pull request
        </a>
      )}
      {preview.fqdn && (
        <a
          href={preview.fqdn}
          target="_blank"
          rel="noreferrer"
          className="min-w-0 truncate font-mono text-xs text-emerald-400 underline-offset-2 hover:underline"
        >
          {preview.fqdn}
        </a>
      )}
      <div className="ml-auto flex shrink-0 gap-2">
        <button
          type="button"
          className={`${btnGhost} py-1 text-xs`}
          disabled={redeploy.isPending}
          onClick={() => redeploy.mutate()}
        >
          {redeploy.isPending ? 'Queueing…' : 'Redeploy'}
        </button>
        <button
          type="button"
          className={`${btnDanger} py-1 text-xs`}
          disabled={cleanup.isPending}
          onClick={() => cleanup.mutate()}
        >
          {cleanup.isPending ? 'Cleaning…' : 'Cleanup'}
        </button>
      </div>
    </div>
  )
}

export default function ApplicationPreviews() {
  const { app } = useApplication()

  const previews = useQuery({
    queryKey: ['application-previews', app.uuid],
    queryFn: () => api.get<Preview[]>(`/applications/${app.uuid}/previews`),
    refetchInterval: 10_000,
  })

  return (
    <div className="flex max-w-3xl flex-col gap-4">
      <SectionTitle>Preview deployments</SectionTitle>
      <ErrorNote error={previews.error} />
      <div className="flex flex-col gap-2">
        {previews.data?.map((p) => <PreviewRow key={p.uuid} appUuid={app.uuid} preview={p} />)}
        {previews.data?.length === 0 && (
          <p className="text-sm text-zinc-500">
            No preview deployments. Open a pull request to trigger one.
          </p>
        )}
      </div>
    </div>
  )
}
