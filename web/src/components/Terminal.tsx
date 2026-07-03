import { useEffect, useRef, useState } from 'react'
import { Terminal as XTerm } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'
import {
  checkActiveFrame,
  classifyServerMessage,
  commandFrame,
  formatRemaining,
  HEARTBEAT_MS,
  messageFrame,
  pingFrame,
  remainingSeconds,
  resizeFrame,
  sessionLevel,
  terminalWsUrl,
  type SessionLevel,
} from '../lib/terminal'

export interface TerminalProps {
  /** Target descriptor: `host:<uuid>` or `container:<uuid>:<name>`. */
  target: string
  /** Optional test seam: override the WebSocket URL. */
  url?: string
  height?: number
}

const MAX_RECONNECTS = 10
const BASE_RECONNECT_MS = 1_000
const MAX_RECONNECT_MS = 30_000

type ConnState = 'connecting' | 'connected' | 'closed'

const levelClass: Record<SessionLevel, string> = {
  normal: 'border-zinc-700 bg-black/60 text-zinc-300',
  warn: 'border-amber-500/40 bg-amber-950/70 text-amber-200',
  danger: 'border-red-500/40 bg-red-950/80 text-red-200',
}

/**
 * Interactive terminal: opens `/terminal/ws`, sends `{command:[target]}`, streams
 * PTY output into xterm.js, forwards keystrokes as `{message}` frames, syncs size
 * with `{resize}`, keepalives with `{ping}`, reconnects on transient drops, and
 * shows an 8h session countdown (warn 30m / danger 5m).
 */
export function Terminal({ target, url, height = 460 }: TerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const [state, setState] = useState<ConnState>('connecting')
  const [active, setActive] = useState(false)
  const [remaining, setRemaining] = useState<number | null>(null)
  const [message, setMessage] = useState<string | null>(null)

  useEffect(() => {
    const term = new XTerm({
      cols: 80,
      rows: 30,
      convertEol: true,
      cursorBlink: true,
      fontFamily: '"SFMono-Regular", Menlo, Monaco, Consolas, "Liberation Mono", monospace',
      fontSize: 13,
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    if (containerRef.current) term.open(containerRef.current)

    const decoder = new TextDecoder()
    let socket: WebSocket | null = null
    let mounted = true
    let userClosed = false
    let reconnectAttempts = 0
    let heartbeat: ReturnType<typeof setInterval> | null = null
    let reconnect: ReturnType<typeof setTimeout> | null = null
    let countdown: ReturnType<typeof setInterval> | null = null
    let startedAt: number | null = null

    const send = (frame: unknown) => {
      if (socket && socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify(frame))
      }
    }

    const doResize = () => {
      if (!active && !term.element) return
      try {
        fit.fit()
        send(resizeFrame(term.cols, term.rows))
      } catch {
        // terminal not yet measurable; ignore
      }
    }

    const stopCountdown = () => {
      if (countdown) clearInterval(countdown)
      countdown = null
      startedAt = null
      setRemaining(null)
    }

    const startCountdown = () => {
      stopCountdown()
      startedAt = Date.now()
      setRemaining(remainingSeconds(startedAt))
      countdown = setInterval(() => {
        if (startedAt) setRemaining(remainingSeconds(startedAt))
      }, 1_000)
    }

    const clearTimers = () => {
      if (heartbeat) clearInterval(heartbeat)
      if (reconnect) clearTimeout(reconnect)
      heartbeat = null
      reconnect = null
    }

    const scheduleReconnect = () => {
      if (userClosed || !mounted || reconnectAttempts >= MAX_RECONNECTS) return
      reconnectAttempts += 1
      const delay = Math.min(
        BASE_RECONNECT_MS * 2 ** (reconnectAttempts - 1) + Math.random() * 500,
        MAX_RECONNECT_MS,
      )
      reconnect = setTimeout(connect, delay)
    }

    const onMessage = (ev: MessageEvent) => {
      const msg = classifyServerMessage(ev.data)
      switch (msg.kind) {
        case 'pong':
          return
        case 'ready':
          setActive(true)
          setMessage(null)
          startCountdown()
          doResize()
          term.focus()
          return
        case 'exited':
          setActive(false)
          stopCountdown()
          setMessage('(session ended)')
          return
        case 'unprocessable':
          setActive(false)
          stopCountdown()
          setMessage('(could not start terminal — check the server or container)')
          return
        case 'data':
          term.write(typeof msg.data === 'string' ? msg.data : decoder.decode(msg.data))
      }
    }

    function connect() {
      setState('connecting')
      const sock = new WebSocket(url ?? terminalWsUrl())
      sock.binaryType = 'arraybuffer'
      socket = sock

      sock.onopen = () => {
        if (!mounted) return
        setState('connected')
        reconnectAttempts = 0
        // (Re)spawn the PTY for the current target.
        send(commandFrame(target))
        if (!heartbeat) heartbeat = setInterval(() => send(pingFrame()), HEARTBEAT_MS)
      }
      sock.onmessage = onMessage
      sock.onerror = () => {
        /* onclose handles retry */
      }
      sock.onclose = () => {
        socket = null
        setState('closed')
        setActive(false)
        stopCountdown()
        clearTimers()
        if (!userClosed) scheduleReconnect()
      }
    }

    const disposeData = term.onData((data) => send(messageFrame(data)))

    const observer =
      typeof ResizeObserver !== 'undefined' ? new ResizeObserver(() => doResize()) : null
    if (observer && containerRef.current) observer.observe(containerRef.current)
    window.addEventListener('resize', doResize)

    connect()

    return () => {
      mounted = false
      userClosed = true
      send(checkActiveFrame())
      clearTimers()
      stopCountdown()
      window.removeEventListener('resize', doResize)
      observer?.disconnect()
      disposeData.dispose()
      if (socket) socket.close(1000, 'client cleanup')
      term.dispose()
    }
  }, [target, url])

  const level = remaining === null ? 'normal' : sessionLevel(remaining)

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-3 text-xs text-zinc-400">
        <span className="flex items-center gap-1.5">
          <span
            className={`h-1.5 w-1.5 rounded-full ${
              state === 'connected' ? (active ? 'bg-emerald-400' : 'bg-amber-400') : 'bg-red-400'
            }`}
          />
          {state === 'connected' ? (active ? 'connected' : 'connecting shell…') : state}
        </span>
        {remaining !== null && (
          <span
            data-testid="session-timer"
            className={`rounded border px-2 py-0.5 font-mono ${levelClass[level]}`}
          >
            Session expires in {formatRemaining(remaining)}
          </span>
        )}
        {message && <span className="text-zinc-500">{message}</span>}
      </div>
      <div
        ref={containerRef}
        role="group"
        aria-label="terminal"
        style={{ height }}
        className="overflow-hidden rounded-lg border border-zinc-800 bg-black p-2"
      />
    </div>
  )
}
