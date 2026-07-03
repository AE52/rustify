import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router'
import { api, type Team } from '../api/client'

/**
 * Top-nav team switcher: shows the active team (`GET /teams/current`) and, on
 * open, the teams the user belongs to (`GET /teams`). Selecting one posts
 * `POST /teams/{id}/switch`, then invalidates every query so the whole app
 * re-reads under the new active team.
 */
export function TeamSwitcher() {
  const [open, setOpen] = useState(false)
  const queryClient = useQueryClient()
  const navigate = useNavigate()

  const current = useQuery({
    queryKey: ['team', 'current'],
    queryFn: () => api.get<Team>('/teams/current'),
    staleTime: 30_000,
  })
  const teams = useQuery({
    queryKey: ['teams'],
    queryFn: () => api.get<Team[]>('/teams'),
    enabled: open,
  })

  const switchTeam = useMutation({
    mutationFn: (id: number) => api.post<Team>(`/teams/${id}/switch`),
    onSuccess: () => {
      setOpen(false)
      queryClient.invalidateQueries()
      navigate('/')
    },
  })

  return (
    <div className="relative">
      <button
        type="button"
        aria-label="Switch team"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center justify-between gap-2 rounded-md border border-zinc-800 bg-zinc-900/60 px-3 py-1.5 text-left text-sm text-zinc-200 hover:bg-zinc-800"
      >
        <span className="truncate">{current.data?.name ?? 'Team'}</span>
        <span className="text-zinc-500">▾</span>
      </button>
      {open && (
        <div
          role="menu"
          className="absolute z-10 mt-1 w-full overflow-hidden rounded-md border border-zinc-800 bg-zinc-900 shadow-lg"
        >
          {teams.isPending && <div className="px-3 py-2 text-xs text-zinc-500">Loading…</div>}
          {teams.data?.map((t) => (
            <button
              key={t.id}
              type="button"
              role="menuitem"
              disabled={switchTeam.isPending}
              onClick={() => switchTeam.mutate(t.id)}
              className={`flex w-full items-center justify-between px-3 py-1.5 text-left text-sm hover:bg-zinc-800 ${
                t.id === current.data?.id ? 'text-zinc-100' : 'text-zinc-400'
              }`}
            >
              <span className="truncate">{t.name}</span>
              {t.id === current.data?.id && <span className="text-emerald-400">✓</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
