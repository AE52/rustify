import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router'
import { api, type Database } from '../../api/client'
import { engineLabel } from '../../lib/engines'
import { StatusBadge } from '../../components/StatusBadge'
import { btnPrimary, ErrorNote, PageTitle } from '../../components/ui'

export default function DatabasesList() {
  const databases = useQuery({
    queryKey: ['databases'],
    queryFn: () => api.get<Database[]>('/databases'),
    refetchInterval: 15_000,
  })

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center justify-between">
        <PageTitle>Databases</PageTitle>
        <Link to="/databases/new" className={btnPrimary}>
          New Resource
        </Link>
      </div>

      <ErrorNote error={databases.error} />
      {databases.isPending && <p className="text-sm text-zinc-500">Loading…</p>}
      <div className="flex flex-col gap-2">
        {databases.data?.map((d) => (
          <Link
            key={d.uuid}
            to={`/databases/${d.uuid}`}
            className="flex items-center justify-between gap-3 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-2.5 hover:border-zinc-600"
          >
            <div className="min-w-0">
              <span className="font-medium text-zinc-100">{d.name}</span>
              <span className="ml-3 text-xs text-zinc-500">{engineLabel(d.engine)}</span>
              <span className="ml-3 truncate font-mono text-xs text-zinc-600">{d.image}</span>
            </div>
            <StatusBadge status={d.status} />
          </Link>
        ))}
        {databases.data?.length === 0 && (
          <p className="text-sm text-zinc-500">
            No databases yet. Create one with <span className="text-zinc-300">New Resource</span>.
          </p>
        )}
      </div>
    </div>
  )
}
