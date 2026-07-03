import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, render, screen } from '@testing-library/react'
import { Terminal } from './Terminal'
import { commandFrame, messageFrame } from '../lib/terminal'

// Shared state the xterm mock records into, readable from the tests.
const h = vi.hoisted(() => ({
  onDataCb: null as ((d: string) => void) | null,
  writes: [] as string[],
  focusCount: 0,
  reset() {
    this.onDataCb = null
    this.writes = []
    this.focusCount = 0
  },
}))

vi.mock('@xterm/xterm/css/xterm.css', () => ({}))
vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit() {}
  },
}))
vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80
    rows = 30
    element = {}
    loadAddon() {}
    open() {}
    focus() {
      h.focusCount++
    }
    write(d: string) {
      h.writes.push(d)
    }
    resize() {}
    dispose() {}
    onData(cb: (d: string) => void) {
      h.onDataCb = cb
      return { dispose() {} }
    }
  },
}))

class FakeWS {
  static instances: FakeWS[] = []
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3

  url: string
  binaryType = ''
  readyState = FakeWS.CONNECTING
  sent: string[] = []
  onopen: (() => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null

  constructor(url: string) {
    this.url = url
    FakeWS.instances.push(this)
  }
  send(data: string) {
    this.sent.push(data)
  }
  close() {
    this.readyState = FakeWS.CLOSED
    this.onclose?.()
  }
  // test helpers
  triggerOpen() {
    this.readyState = FakeWS.OPEN
    this.onopen?.()
  }
  triggerMessage(data: unknown) {
    this.onmessage?.({ data })
  }
}

beforeEach(() => {
  h.reset()
  FakeWS.instances = []
  vi.stubGlobal('WebSocket', FakeWS)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function connect(target = 'host:srv1') {
  render(<Terminal target={target} url="ws://test/terminal/ws" />)
  const ws = FakeWS.instances[0]
  act(() => ws.triggerOpen())
  return ws
}

describe('<Terminal /> frame handling', () => {
  it('sends the command frame on open with the target', () => {
    const ws = connect('container:srv1:web-1')
    expect(ws.sent).toContain(JSON.stringify(commandFrame('container:srv1:web-1')))
    expect(ws.binaryType).toBe('arraybuffer')
  })

  it('forwards terminal keystrokes as message frames', () => {
    const ws = connect()
    act(() => h.onDataCb?.('l'))
    act(() => h.onDataCb?.('s'))
    expect(ws.sent).toContain(JSON.stringify(messageFrame('l')))
    expect(ws.sent).toContain(JSON.stringify(messageFrame('s')))
  })

  it('starts the session countdown and focuses on pty-ready', () => {
    const ws = connect()
    act(() => ws.triggerMessage('pty-ready'))
    expect(screen.getByTestId('session-timer')).toBeInTheDocument()
    expect(h.focusCount).toBeGreaterThan(0)
  })

  it('writes binary PTY output into the terminal', () => {
    const ws = connect()
    act(() => ws.triggerMessage(new Uint8Array([104, 105]).buffer))
    expect(h.writes.join('')).toContain('hi')
  })

  it('ignores pong keepalives (no terminal write)', () => {
    const ws = connect()
    const before = h.writes.length
    act(() => ws.triggerMessage('pong'))
    expect(h.writes.length).toBe(before)
  })

  it('surfaces unprocessable as an error message', () => {
    const ws = connect()
    act(() => ws.triggerMessage('unprocessable'))
    expect(screen.getByText(/could not start terminal/i)).toBeInTheDocument()
  })
})
