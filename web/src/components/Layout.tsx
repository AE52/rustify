import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, Navigate, NavLink, Outlet, useNavigate } from 'react-router'
import { api, ApiError, type User } from '../api/client'
import { errText } from './ui'

const navCls = ({ isActive }: { isActive: boolean }) =>
  `rounded-md px-3 py-1.5 text-sm ${
    isActive ? 'bg-zinc-800 font-medium text-zinc-100' : 'text-zinc-400 hover:bg-zinc-900 hover:text-zinc-200'
  }`

export function Layout() {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const me = useQuery({
    queryKey: ['me'],
    queryFn: () => api.get<User>('/auth/me'),
    retry: false,
    staleTime: 60_000,
  })

  if (me.isError) {
    if (me.error instanceof ApiError && me.error.status === 401) {
      return <Navigate to="/login" replace />
    }
    return (
      <div className="grid min-h-screen place-items-center">
        <div className="text-center text-sm text-zinc-400">
          <p className="mb-2 text-red-400">{errText(me.error)}</p>
          <button
            type="button"
            onClick={() => me.refetch()}
            className="rounded-md border border-zinc-700 px-3 py-1.5 hover:bg-zinc-800"
          >
            Retry
          </button>
        </div>
      </div>
    )
  }

  if (me.isPending) {
    return <div className="grid min-h-screen place-items-center text-sm text-zinc-500">Loading…</div>
  }

  const logout = async () => {
    try {
      await api.post('/auth/logout')
    } finally {
      queryClient.clear()
      navigate('/login')
    }
  }

  return (
    <div className="flex min-h-screen">
      <aside className="flex w-52 shrink-0 flex-col border-r border-zinc-800 p-4">
        <Link to="/" className="mb-6 px-3 text-lg font-bold tracking-tight text-zinc-100">
          rustify
        </Link>
        <nav className="flex flex-col gap-1">
          <NavLink to="/" end className={navCls}>
            Dashboard
          </NavLink>
          <NavLink to="/databases" className={navCls}>
            Databases
          </NavLink>
          <NavLink to="/services" className={navCls}>
            Services
          </NavLink>
          <NavLink to="/sources" className={navCls}>
            Sources
          </NavLink>
          <NavLink to="/notifications" className={navCls}>
            Notifications
          </NavLink>
          <NavLink to="/onboarding" className={navCls}>
            Onboarding
          </NavLink>
          <NavLink to="/settings" className={navCls}>
            Settings
          </NavLink>
        </nav>
        <div className="mt-auto flex flex-col gap-1 px-3 text-xs text-zinc-500">
          <span className="truncate" title={me.data.email}>
            {me.data.email}
          </span>
          <button
            type="button"
            onClick={logout}
            className="w-fit text-zinc-400 underline-offset-2 hover:text-zinc-200 hover:underline"
          >
            Log out
          </button>
        </div>
      </aside>
      <main className="min-w-0 flex-1 px-6 py-6 lg:px-10">
        <div className="mx-auto max-w-5xl">
          <Outlet />
        </div>
      </main>
    </div>
  )
}
