/**
 * Reconnecting WebSocket client for the C4 envelope.
 *
 * Server → client: `{ channel, event, data }`
 * Client → server: `{ action: "subscribe" | "unsubscribe", channel }`
 * Auth rides on the session cookie at upgrade time.
 */

export interface WsEnvelope {
  channel: string
  event: string
  data: unknown
}

export type WsHandler = (envelope: WsEnvelope) => void

const MAX_RECONNECT_ATTEMPTS = 10
const BASE_DELAY_MS = 500
const MAX_DELAY_MS = 15_000

function defaultUrl(): string {
  const proto = window.location.protocol === 'https:' ? 'wss' : 'ws'
  return `${proto}://${window.location.host}/ws`
}

export class WsClient {
  private url: string | undefined
  private sock: WebSocket | null = null
  private handlers = new Map<string, Set<WsHandler>>()
  private reconnectAttempts = 0
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private closedByUser = false
  private openListeners = new Set<() => void>()

  constructor(url?: string) {
    this.url = url
  }

  /**
   * Subscribe to a channel (`deployment:<uuid>`, `team:<uuid>`, `server:<uuid>`).
   * Returns an unsubscribe function. The socket connects lazily on first use.
   */
  subscribe(channel: string, handler: WsHandler): () => void {
    let set = this.handlers.get(channel)
    const isNewChannel = !set
    if (!set) {
      set = new Set()
      this.handlers.set(channel, set)
    }
    set.add(handler)

    this.closedByUser = false
    this.connect()
    if (isNewChannel && this.isOpen()) {
      this.send({ action: 'subscribe', channel })
    }

    return () => {
      const current = this.handlers.get(channel)
      if (!current) return
      current.delete(handler)
      if (current.size === 0) {
        this.handlers.delete(channel)
        if (this.isOpen()) {
          this.send({ action: 'unsubscribe', channel })
        }
      }
    }
  }

  /** Fires after every (re)connect; used by views to refetch missed state. */
  onOpen(listener: () => void): () => void {
    this.openListeners.add(listener)
    return () => {
      this.openListeners.delete(listener)
    }
  }

  close(): void {
    this.closedByUser = true
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    const sock = this.sock
    this.sock = null
    if (sock) {
      sock.onclose = null
      sock.close()
    }
  }

  private isOpen(): boolean {
    return this.sock !== null && this.sock.readyState === 1 // WebSocket.OPEN
  }

  private send(payload: { action: 'subscribe' | 'unsubscribe'; channel: string }): void {
    this.sock?.send(JSON.stringify(payload))
  }

  private connect(): void {
    if (this.sock !== null || this.closedByUser) return

    const sock = new WebSocket(this.url ?? defaultUrl())
    this.sock = sock

    sock.onopen = () => {
      this.reconnectAttempts = 0
      for (const channel of this.handlers.keys()) {
        this.send({ action: 'subscribe', channel })
      }
      for (const listener of this.openListeners) {
        listener()
      }
    }

    sock.onmessage = (ev: MessageEvent) => {
      let envelope: WsEnvelope
      try {
        envelope = JSON.parse(String(ev.data)) as WsEnvelope
      } catch {
        return
      }
      if (!envelope || typeof envelope.channel !== 'string') return
      const set = this.handlers.get(envelope.channel)
      if (!set) return
      for (const handler of set) {
        handler(envelope)
      }
    }

    sock.onclose = () => {
      this.sock = null
      this.scheduleReconnect()
    }

    sock.onerror = () => {
      // onclose follows; nothing to do here
    }
  }

  private scheduleReconnect(): void {
    if (this.closedByUser || this.reconnectTimer !== null) return
    if (this.handlers.size === 0) return
    if (this.reconnectAttempts >= MAX_RECONNECT_ATTEMPTS) return

    this.reconnectAttempts += 1
    const delay = Math.min(
      BASE_DELAY_MS * 2 ** (this.reconnectAttempts - 1),
      MAX_DELAY_MS,
    )
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      this.connect()
    }, delay)
  }
}

/** App-wide singleton; lazy, so importing this module opens no socket. */
export const ws = new WsClient()
