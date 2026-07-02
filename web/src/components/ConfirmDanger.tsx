import { useState } from 'react'
import { btnDanger, btnGhost, inputCls } from './ui'

export interface ConfirmDangerProps {
  /** Button label, e.g. "Delete server". */
  label: string
  /** The exact text the user must type to arm the action (usually the resource name). */
  confirmText: string
  description?: string
  busy?: boolean
  onConfirm: () => void
}

/**
 * Destructive action guard: expands into a type-to-confirm prompt; the
 * action stays disabled until the user types the resource name exactly.
 */
export function ConfirmDanger({ label, confirmText, description, busy = false, onConfirm }: ConfirmDangerProps) {
  const [open, setOpen] = useState(false)
  const [typed, setTyped] = useState('')
  const armed = typed === confirmText

  if (!open) {
    return (
      <button type="button" className={btnDanger} onClick={() => setOpen(true)}>
        {label}
      </button>
    )
  }

  return (
    <div className="flex flex-col gap-2 rounded-lg border border-red-900/60 bg-red-950/20 p-3">
      <p className="text-sm text-zinc-300">
        {description ?? 'This action cannot be undone.'} Type{' '}
        <code className="rounded bg-zinc-800 px-1 py-0.5 text-xs text-red-300">{confirmText}</code> to
        confirm.
      </p>
      <input
        aria-label="confirm name"
        className={inputCls}
        value={typed}
        onChange={(e) => setTyped(e.target.value)}
        placeholder={confirmText}
      />
      <div className="flex gap-2">
        <button type="button" className={btnDanger} disabled={!armed || busy} onClick={onConfirm}>
          {busy ? 'Working…' : label}
        </button>
        <button
          type="button"
          className={btnGhost}
          onClick={() => {
            setOpen(false)
            setTyped('')
          }}
        >
          Cancel
        </button>
      </div>
    </div>
  )
}
