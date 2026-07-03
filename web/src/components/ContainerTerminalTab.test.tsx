import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { ContainerTerminalTab } from './ContainerTerminalTab'
import { commandFrame, containerTarget } from '../lib/terminal'
import { renderApp } from '../test/harness'

vi.mock('@xterm/xterm/css/xterm.css', () => ({}))
vi.mock('@xterm/addon-fit', () => ({ FitAddon: class { fit() {} } }))
vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80
    rows = 30
    element = {}
    loadAddon() {}
    open() {}
    focus() {}
    write() {}
    resize() {}
    dispose() {}
    onData() {
      return { dispose() {} }
    }
  },
}))

class FakeWS {
  static instances: FakeWS[] = []
  static OPEN = 1
  url: string
  binaryType = ''
  readyState = 0
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
    this.readyState = 3
    this.onclose?.()
  }
  triggerOpen() {
    this.readyState = FakeWS.OPEN
    this.onopen?.()
  }
}

beforeEach(() => {
  FakeWS.instances = []
  vi.stubGlobal('WebSocket', FakeWS)
})
afterEach(() => vi.unstubAllGlobals())

describe('<ContainerTerminalTab />', () => {
  it('opens a terminal WebSocket and sends the container command on connect', async () => {
    const user = userEvent.setup()
    renderApp(<ContainerTerminalTab serverUuid="srv1" defaultContainer="db-abc" />)

    // No socket until the operator clicks Connect.
    expect(FakeWS.instances).toHaveLength(0)

    await user.click(screen.getByRole('button', { name: /connect/i }))

    // The Terminal mounts and dials /terminal/ws.
    expect(FakeWS.instances).toHaveLength(1)
    expect(FakeWS.instances[0].url).toContain('/terminal/ws')

    act(() => FakeWS.instances[0].triggerOpen())
    expect(FakeWS.instances[0].sent).toContain(
      JSON.stringify(commandFrame(containerTarget('srv1', 'db-abc'))),
    )
  })
})
