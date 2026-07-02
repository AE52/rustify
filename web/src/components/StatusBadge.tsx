export type BadgeColor = 'green' | 'yellow' | 'red' | 'gray' | 'blue'

/**
 * Maps a resource/deployment status string to a badge color.
 * Container statuses follow Coolify's `state:health` convention
 * (e.g. `running:healthy`, `running:unhealthy`, `exited`).
 */
export function statusColor(status: string): BadgeColor {
  const [state, health] = status.split(':')
  switch (state) {
    case 'running':
      return health === undefined || health === 'healthy' ? 'green' : 'yellow'
    case 'finished':
      return 'green'
    case 'starting':
    case 'restarting':
    case 'in_progress':
      return 'yellow'
    case 'crashed':
    case 'failed':
    case 'unhealthy':
      return 'red'
    case 'queued':
      return 'blue'
    default:
      // exited, cancelled, paused, unknown, ...
      return 'gray'
  }
}

const COLOR_CLASSES: Record<BadgeColor, string> = {
  green: 'bg-emerald-500/10 text-emerald-400 border-emerald-500/30',
  yellow: 'bg-amber-500/10 text-amber-400 border-amber-500/30',
  red: 'bg-red-500/10 text-red-400 border-red-500/30',
  gray: 'bg-zinc-500/10 text-zinc-400 border-zinc-500/30',
  blue: 'bg-sky-500/10 text-sky-400 border-sky-500/30',
}

const DOT_CLASSES: Record<BadgeColor, string> = {
  green: 'bg-emerald-400',
  yellow: 'bg-amber-400',
  red: 'bg-red-400',
  gray: 'bg-zinc-400',
  blue: 'bg-sky-400',
}

function label(status: string): string {
  const state = status.split(':')[0].replace(/_/g, ' ')
  return state.charAt(0).toUpperCase() + state.slice(1)
}

export function StatusBadge({ status }: { status: string }) {
  const color = statusColor(status)
  return (
    <span
      data-testid="status-badge"
      data-color={color}
      data-status={status}
      title={status}
      className={`inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs font-medium ${COLOR_CLASSES[color]}`}
    >
      <span
        className={`h-1.5 w-1.5 rounded-full ${DOT_CLASSES[color]} ${
          color === 'yellow' || color === 'blue' ? 'animate-pulse' : ''
        }`}
      />
      {label(status)}
    </span>
  )
}
