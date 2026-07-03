import '@testing-library/jest-dom/vitest'
import { cleanup } from '@testing-library/react'
import { afterEach } from 'vitest'

// jsdom has no WebSocket; the realtime client (`api/ws`) constructs one lazily
// on subscribe. Provide an inert baseline so components that subscribe render
// without throwing. Tests that exercise the client itself override this via
// `vi.stubGlobal`.
class InertWebSocket {
  static readonly CONNECTING = 0
  static readonly OPEN = 1
  static readonly CLOSING = 2
  static readonly CLOSED = 3
  readyState = 0
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: ((ev: { data: string }) => void) | null = null
  send(): void {}
  close(): void {}
}

if (!('WebSocket' in globalThis)) {
  ;(globalThis as unknown as { WebSocket: unknown }).WebSocket = InertWebSocket
}

afterEach(() => {
  cleanup()
})
