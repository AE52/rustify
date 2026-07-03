import { useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate, useParams } from 'react-router'
import {
  api,
  type BranchesResponse,
  type GithubApp,
  type RepositoriesResponse,
} from '../../api/client'
import { parseBranch, parseRepo, type GithubRepo } from '../../lib/github'
import { ConfirmDanger } from '../../components/ConfirmDanger'
import {
  cardCls,
  ErrorNote,
  Field,
  PageTitle,
  SectionTitle,
  selectCls,
} from '../../components/ui'

/** Repo + branch picker for a GitHub App (source detail). */
export function RepoBranchPicker({ appUuid }: { appUuid: string }) {
  const [selected, setSelected] = useState<GithubRepo | null>(null)
  const [branch, setBranch] = useState('')

  const repos = useQuery({
    queryKey: ['github-app', appUuid, 'repositories'],
    queryFn: () => api.get<RepositoriesResponse>(`/github-apps/${appUuid}/repositories`),
  })

  const parsedRepos = useMemo(
    () => (repos.data?.repositories ?? []).map(parseRepo).filter((r): r is GithubRepo => r !== null),
    [repos.data],
  )

  const branches = useQuery({
    queryKey: ['github-app', appUuid, 'branches', selected?.full_name],
    queryFn: () =>
      api.get<BranchesResponse>(
        `/github-apps/${appUuid}/repositories/${selected!.owner}/${selected!.name}/branches`,
      ),
    enabled: Boolean(selected),
  })

  const parsedBranches = useMemo(
    () => (branches.data?.branches ?? []).map(parseBranch).filter((b): b is string => b !== null),
    [branches.data],
  )

  return (
    <div className={`${cardCls} flex flex-col gap-3`}>
      <SectionTitle>Repositories</SectionTitle>
      {repos.isPending && <p className="text-sm text-zinc-500">Loading repositories…</p>}
      <ErrorNote error={repos.error} />
      {repos.data && (
        <Field label="Repository">
          <select
            aria-label="Repository"
            className={selectCls}
            value={selected?.full_name ?? ''}
            onChange={(e) => {
              const repo = parsedRepos.find((r) => r.full_name === e.target.value) ?? null
              setSelected(repo)
              setBranch(repo?.default_branch ?? '')
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
      {selected && (
        <Field label="Branch">
          <select
            aria-label="Branch"
            className={selectCls}
            value={branch}
            onChange={(e) => setBranch(e.target.value)}
          >
            {branches.isPending && <option>Loading…</option>}
            {parsedBranches.map((b) => (
              <option key={b} value={b}>
                {b}
              </option>
            ))}
          </select>
        </Field>
      )}
      <ErrorNote error={branches.error} />
    </div>
  )
}

export default function SourceDetailPage() {
  const { uuid = '' } = useParams()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const app = useQuery({
    queryKey: ['github-app', uuid],
    queryFn: () => api.get<GithubApp>(`/github-apps/${uuid}`),
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/github-apps/${uuid}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['github-apps'] })
      navigate('/sources')
    },
  })

  if (app.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (app.isError) return <ErrorNote error={app.error} />

  const g = app.data

  return (
    <div className="flex flex-col gap-6">
      <div>
        <PageTitle>{g.name}</PageTitle>
        <p className="mt-1 truncate font-mono text-xs text-zinc-500">{g.html_url}</p>
      </div>

      <div className={`${cardCls} grid max-w-2xl grid-cols-2 gap-3 text-sm`}>
        <div>
          <span className="text-xs text-zinc-500">App ID</span>
          <p className="font-mono text-zinc-200">{g.app_id ?? '—'}</p>
        </div>
        <div>
          <span className="text-xs text-zinc-500">Installation ID</span>
          <p className="font-mono text-zinc-200">{g.installation_id ?? '—'}</p>
        </div>
        <div>
          <span className="text-xs text-zinc-500">Client ID</span>
          <p className="font-mono text-zinc-200">{g.client_id ?? '—'}</p>
        </div>
        <div>
          <span className="text-xs text-zinc-500">Organization</span>
          <p className="font-mono text-zinc-200">{g.organization ?? '—'}</p>
        </div>
      </div>

      {g.installation_id ? (
        <RepoBranchPicker appUuid={uuid} />
      ) : (
        <p className="text-sm text-amber-400">
          This App is not installed yet. Complete the GitHub installation to browse repositories.
        </p>
      )}

      <section className="max-w-2xl">
        <SectionTitle>Danger zone</SectionTitle>
        <ConfirmDanger
          label="Delete GitHub App"
          confirmText={g.name}
          description="Removes this source. Applications using it will lose their git source."
          busy={remove.isPending}
          onConfirm={() => remove.mutate()}
        />
        <ErrorNote error={remove.error} />
      </section>
    </div>
  )
}
