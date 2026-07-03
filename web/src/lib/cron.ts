/**
 * Cron frequency helpers for scheduled tasks and backups.
 *
 * Mirrors `rustify_core::cron::VALID_CRON_STRINGS`: a task's frequency is
 * either one of the named aliases or a raw 5-field cron expression. Used to
 * validate the create form before hitting the API.
 */

/** Named aliases accepted by the backend (`VALID_CRON_STRINGS`). */
export const CRON_ALIASES: { value: string; label: string; cron: string }[] = [
  { value: 'every_minute', label: 'Every minute', cron: '* * * * *' },
  { value: 'hourly', label: 'Hourly', cron: '0 * * * *' },
  { value: 'daily', label: 'Daily (midnight)', cron: '0 0 * * *' },
  { value: 'weekly', label: 'Weekly (Sunday)', cron: '0 0 * * 0' },
  { value: 'monthly', label: 'Monthly (1st)', cron: '0 0 1 * *' },
  { value: 'yearly', label: 'Yearly (Jan 1)', cron: '0 0 1 1 *' },
]

const ALIAS_SET = new Set([
  ...CRON_ALIASES.map((a) => a.value),
  '@hourly',
  '@daily',
  '@weekly',
  '@monthly',
  '@yearly',
])

/** A single cron field: numbers, `*`, ranges, steps and lists. */
const FIELD = /^(\*|(\d+)(-\d+)?)(\/\d+)?(,(\*|(\d+)(-\d+)?)(\/\d+)?)*$/

/**
 * Whether `frequency` is a valid backend frequency: a known alias, or a
 * whitespace-separated 5-field cron expression with syntactically valid fields.
 */
export function isValidFrequency(frequency: string): boolean {
  const trimmed = frequency.trim()
  if (trimmed.length === 0) return false
  if (ALIAS_SET.has(trimmed)) return true
  const fields = trimmed.split(/\s+/)
  if (fields.length !== 5) return false
  return fields.every((f) => FIELD.test(f))
}
