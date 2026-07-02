import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { WsClient } from './ws'

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  static reset() {
    FakeWebSocket.instances = []
  }

  url: string
  readyState = 0 // CONNECTING
  sent: string[] = []
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: ((ev: { data: string }) => void) | null = null

  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }

  send(data: string) {
    this.sent.push(data)
  }

  close() {
    this.readyState = 3
    this.onclose?.()
  }

  // test helpers
  open() {
    this.readyState = 1
    this.onopen?.()
  }

  drop() {
    this.readyState = 3
    this.onclose?.()
  }

  message(obj: unknown) {
    this.onmessage?.({ data: JSON.stringify(obj) })
  }
}

const last = () => FakeWebSocket.instances[FakeWebSocket.instances.length - 1]

describe('ws client', () => {
  beforeEach(() => {
    FakeWebSocket.reset()
    vi.stubGlobal('WebSocket', FakeWebSocket)
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('sends subscribe action per C4 once connected', () => {
    const client = new WsClient('ws://test/ws')
    client.subscribe('deployment:abc', () => {})

    expect(FakeWebSocket.instances).toHaveLength(1)
    last().open()

    expect(last().sent.map((s) => JSON.parse(s))).toEqual([
      { action: 'subscribe', channel: 'deployment:abc' },
    ])
    client.close()
  })

  it('dispatches envelopes to channel handlers', () => {
    const client = new WsClient('ws://test/ws')
    const seen: unknown[] = []
    client.subscribe('deployment:abc', (env) => seen.push(env))
    client.subscribe('team:t1', () => {
      throw new Error('wrong channel')
    })
    last().open()

    last().message({
      channel: 'deployment:abc',
      event: 'deployment_log_appended',
      data: { order: 1 },
    })

    expect(seen).toEqual([
      { channel: 'deployment:abc', event: 'deployment_log_appended', data: { order: 1 } },
    ])
    client.close()
  })

  it('sends unsubscribe when last handler for a channel is removed', () => {
    const client = new WsClient('ws://test/ws')
    const off = client.subscribe('team:t1', () => {})
    last().open()

    off()

    expect(last().sent.map((s) => JSON.parse(s))).toEqual([
      { action: 'subscribe', channel: 'team:t1' },
      { action: 'unsubscribe', channel: 'team:t1' },
    ])
    client.close()
  })

  it('reconnects with backoff and resubscribes', () => {
    const client = new WsClient('ws://test/ws')
    client.subscribe('deployment:abc', () => {})
    last().open()
    expect(FakeWebSocket.instances).toHaveLength(1)

    last().drop()
    vi.advanceTimersByTime(60_000)
    expect(FakeWebSocket.instances).toHaveLength(2)

    last().open()
    expect(last().sent.map((s) => JSON.parse(s))).toEqual([
      { action: 'subscribe', channel: 'deployment:abc' },
    ])
    client.close()
  })

  it('gives up after 10 reconnect attempts', () => {
    const client = new WsClient('ws://test/ws')
    client.subscribe('deployment:abc', () => {})
    last().open()

    // initial connection + 10 retries, each dropped before opening
    for (let i = 0; i < 30; i++) {
      last().drop()
      vi.advanceTimersByTime(120_000)
    }

    expect(FakeWebSocket.instances.length).toBe(11)
    client.close()
  })
})
