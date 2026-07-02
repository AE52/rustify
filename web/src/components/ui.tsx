import type { ReactNode } from 'react'
import { ApiError } from '../api/client'

export const inputCls =
  'w-full rounded-md border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-sm text-zinc-200 placeholder-zinc-600 outline-none focus:border-zinc-500'

export const selectCls =
  'w-full rounded-md border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-sm text-zinc-200 outline-none focus:border-zinc-500'

export const btnPrimary =
  'rounded-md bg-zinc-100 px-3 py-1.5 text-sm font-semibold text-zinc-900 hover:bg-white disabled:cursor-not-allowed disabled:opacity-40'

export const btnGhost =
  'rounded-md border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-40'

export const btnDanger =
  'rounded-md border border-red-900 bg-red-950/40 px-3 py-1.5 text-sm text-red-400 hover:bg-red-900/40 disabled:cursor-not-allowed disabled:opacity-40'

export const cardCls = 'rounded-lg border border-zinc-800 bg-zinc-900/40 p-4'

export function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs font-medium text-zinc-400">{label}</span>
      {children}
    </label>
  )
}

export function SectionTitle({ children }: { children: ReactNode }) {
  return <h2 className="mb-3 text-sm font-semibold tracking-wide text-zinc-200">{children}</h2>
}

export function PageTitle({ children }: { children: ReactNode }) {
  return <h1 className="text-xl font-bold text-zinc-100">{children}</h1>
}

export function errText(e: unknown): string | null {
  if (e == null) return null
  if (e instanceof ApiError) return `${e.message} (${e.code})`
  if (e instanceof Error) return e.message
  return String(e)
}

export function ErrorNote({ error }: { error: unknown }) {
  const text = errText(error)
  if (!text) return null
  return <p className="text-sm text-red-400">{text}</p>
}
