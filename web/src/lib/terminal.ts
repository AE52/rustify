/**
 * Web-terminal wire protocol (client side).
 *
 * Mirrors the in-process PTY handler at `crates/rustify-server/src/terminal.rs`
 * (which itself ports Coolify's `terminal-server.js` protocol). Server → client
 * text sentinels are `pty-ready` / `pty-exited` / `unprocessable` / `pong`;
 * everything else is raw PTY output delivered as binary frames. Client → server
 * frames are single-key JSON objects.
 *
 * The pure helpers here are unit-tested; the React component composes them with
 * xterm.js and a WebSocket.
 */

/** Hard session cap enforced by the server (8h). */
export const MAX_SESSION_SECONDS = 28_800
/** Countdown enters the "warning" band at 30 minutes remaining. */
export const SESSION_WARN_SECONDS = 1_800
/** Countdown enters the "danger" band at 5 minutes remaining. */
export const SESSION_DANGER_SECONDS = 300
/** Application-level keepalive cadence (matches the server heartbeat). */
export const HEARTBEAT_MS = 30_000

export type SessionLevel = 'normal' | 'warn' | 'danger'

/** Build the same-origin terminal WebSocket URL. */
export function terminalWsUrl(): string {
  const proto = window.location.protocol === 'https:' ? 'wss' : 'ws'
  return `${proto}://${window.location.host}/terminal/ws`
}

/** A terminal target descriptor understood by the server's `parse_target`. */
export function hostTarget(serverUuid: string): string {
  return `host:${serverUuid}`
}

export function containerTarget(serverUuid: string, container: string): string {
  return `container:${serverUuid}:${container}`
}

// ----- client → server frame builders -----------------------------------

export const commandFrame = (target: string) => ({ command: [target] })
export const messageFrame = (data: string) => ({ message: data })
export const resizeFrame = (cols: number, rows: number) => ({ resize: { cols, rows } })
export const pingFrame = () => ({ ping: true as const })
export const checkActiveFrame = () => ({ checkActive: 'force' as const })

// ----- server → client message classification ---------------------------

export type ServerMessage =
  | { kind: 'pong' }
  | { kind: 'ready' }
  | { kind: 'exited' }
  | { kind: 'unprocessable' }
  | { kind: 'data'; data: Uint8Array | string }

/**
 * Classify a raw `MessageEvent.data`. Text sentinels map to their kinds; any
 * other payload (string fallback or binary) is PTY output. Callers should set
 * `socket.binaryType = 'arraybuffer'` so binary frames arrive as `ArrayBuffer`.
 */
export function classifyServerMessage(data: unknown): ServerMessage {
  if (typeof data === 'string') {
    switch (data) {
      case 'pong':
        return { kind: 'pong' }
      case 'pty-ready':
        return { kind: 'ready' }
      case 'pty-exited':
        return { kind: 'exited' }
      case 'unprocessable':
        return { kind: 'unprocessable' }
      default:
        return { kind: 'data', data }
    }
  }
  if (data instanceof ArrayBuffer) {
    return { kind: 'data', data: new Uint8Array(data) }
  }
  if (data instanceof Uint8Array) {
    return { kind: 'data', data }
  }
  return { kind: 'data', data: '' }
}

// ----- session countdown -------------------------------------------------

/** Remaining seconds given a session start timestamp (ms since epoch). */
export function remainingSeconds(startedAtMs: number, nowMs: number = Date.now()): number {
  const elapsed = (nowMs - startedAtMs) / 1000
  return Math.max(0, MAX_SESSION_SECONDS - elapsed)
}

/** Countdown severity band, driving the timer badge colour. */
export function sessionLevel(remaining: number): SessionLevel {
  if (remaining <= SESSION_DANGER_SECONDS) return 'danger'
  if (remaining <= SESSION_WARN_SECONDS) return 'warn'
  return 'normal'
}

/** Format remaining seconds as `H:MM:SS` (or `MM:SS` under an hour). */
export function formatRemaining(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = s % 60
  const pad = (n: number) => String(n).padStart(2, '0')
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${pad(m)}:${pad(sec)}`
}
