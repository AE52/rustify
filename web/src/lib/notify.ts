// Notification channels and events, mirroring rustify_core::notify (Channel +
// NotifEvent slugs). The event matrix is `{ [event]: { [channel]: bool } }`.

export const NOTIFY_CHANNELS = [
  'email',
  'discord',
  'telegram',
  'slack',
  'pushover',
  'webhook',
] as const

export type NotifyChannel = (typeof NOTIFY_CHANNELS)[number]

export const CHANNEL_LABELS: Record<NotifyChannel, string> = {
  email: 'Email',
  discord: 'Discord',
  telegram: 'Telegram',
  slack: 'Slack',
  pushover: 'Pushover',
  webhook: 'Webhook',
}

/** Events surfaced in the matrix grid (excludes always-send internal events). */
export const NOTIFY_EVENTS: { slug: string; label: string }[] = [
  { slug: 'deployment_success', label: 'Deployment success' },
  { slug: 'deployment_failure', label: 'Deployment failure' },
  { slug: 'status_change', label: 'Status change' },
  { slug: 'backup_success', label: 'Backup success' },
  { slug: 'backup_failure', label: 'Backup failure' },
  { slug: 'scheduled_task_success', label: 'Scheduled task success' },
  { slug: 'scheduled_task_failure', label: 'Scheduled task failure' },
  { slug: 'docker_cleanup_success', label: 'Docker cleanup success' },
  { slug: 'docker_cleanup_failure', label: 'Docker cleanup failure' },
  { slug: 'server_disk_usage', label: 'Server disk usage' },
  { slug: 'server_reachable', label: 'Server reachable' },
  { slug: 'server_unreachable', label: 'Server unreachable' },
  { slug: 'server_patch', label: 'Server patch' },
  { slug: 'ssl_certificate_renewal', label: 'SSL certificate renewal' },
  { slug: 'api_token_expiring', label: 'API token expiring' },
]

export type EventMatrix = Record<string, Record<string, boolean>>

/** Read a cell from a (possibly partial / unknown-typed) matrix. */
export function matrixCell(matrix: EventMatrix, event: string, channel: string): boolean {
  return Boolean(matrix?.[event]?.[channel])
}

/** Return a new matrix with `[event][channel]` set to `value` (immutably). */
export function setMatrixCell(
  matrix: EventMatrix,
  event: string,
  channel: string,
  value: boolean,
): EventMatrix {
  return {
    ...matrix,
    [event]: { ...(matrix[event] ?? {}), [channel]: value },
  }
}
