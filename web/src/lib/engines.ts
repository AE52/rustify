/**
 * The eight standalone database engines, mirroring
 * `rustify_core::db_engine::ALL_ENGINES`. `port` is the engine's default
 * in-container port (`EngineDescriptor::internal_port`), used to render
 * connection strings.
 */
export interface EngineInfo {
  /** snake_case name accepted by the create API. */
  value: string
  /** Human label for the picker. */
  label: string
  /** Default in-container port. */
  port: number
  /** URI scheme for the connection string, or null for schemeless engines. */
  scheme: string | null
}

export const DATABASE_ENGINES: EngineInfo[] = [
  { value: 'postgresql', label: 'PostgreSQL', port: 5432, scheme: 'postgresql' },
  { value: 'mysql', label: 'MySQL', port: 3306, scheme: 'mysql' },
  { value: 'mariadb', label: 'MariaDB', port: 3306, scheme: 'mysql' },
  { value: 'mongodb', label: 'MongoDB', port: 27017, scheme: 'mongodb' },
  { value: 'redis', label: 'Redis', port: 6379, scheme: 'redis' },
  { value: 'keydb', label: 'KeyDB', port: 6379, scheme: 'redis' },
  { value: 'dragonfly', label: 'Dragonfly', port: 6379, scheme: 'redis' },
  { value: 'clickhouse', label: 'ClickHouse', port: 9000, scheme: 'clickhouse' },
]

export function engineInfo(engine: string): EngineInfo | undefined {
  return DATABASE_ENGINES.find((e) => e.value === engine)
}

export function engineLabel(engine: string): string {
  return engineInfo(engine)?.label ?? engine
}

/**
 * Best-effort connection strings for a database. Credentials are managed
 * server-side and never returned by the API, so the user/password segments are
 * rendered as placeholders. Returns an internal string (container network) and,
 * when the database is publicly exposed, an external one.
 */
export function connectionStrings(db: {
  engine: string
  uuid: string
  is_public: boolean
  public_port?: number | null
}): { label: string; value: string }[] {
  const info = engineInfo(db.engine)
  if (!info) return []
  const host = db.uuid
  const cred = '<user>:<password>'
  const out: { label: string; value: string }[] = []
  const scheme = info.scheme
  const internal = scheme
    ? `${scheme}://${cred}@${host}:${info.port}`
    : `${host}:${info.port}`
  out.push({ label: 'Internal', value: internal })
  if (db.is_public && db.public_port) {
    const external = scheme
      ? `${scheme}://${cred}@<server-ip>:${db.public_port}`
      : `<server-ip>:${db.public_port}`
    out.push({ label: 'Public', value: external })
  }
  return out
}
