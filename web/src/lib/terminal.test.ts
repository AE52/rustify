import { describe, expect, it } from 'vitest'
import {
  checkActiveFrame,
  classifyServerMessage,
  commandFrame,
  containerTarget,
  formatRemaining,
  hostTarget,
  MAX_SESSION_SECONDS,
  messageFrame,
  pingFrame,
  remainingSeconds,
  resizeFrame,
  sessionLevel,
} from './terminal'

describe('frame builders', () => {
  it('wraps a command target in a single-element array', () => {
    expect(commandFrame('host:srv1')).toEqual({ command: ['host:srv1'] })
  })
  it('builds message / resize / ping / checkActive frames', () => {
    expect(messageFrame('ls\r')).toEqual({ message: 'ls\r' })
    expect(resizeFrame(120, 40)).toEqual({ resize: { cols: 120, rows: 40 } })
    expect(pingFrame()).toEqual({ ping: true })
    expect(checkActiveFrame()).toEqual({ checkActive: 'force' })
  })
  it('builds target descriptors the server understands', () => {
    expect(hostTarget('srv1')).toBe('host:srv1')
    expect(containerTarget('srv1', 'web-1')).toBe('container:srv1:web-1')
  })
})

describe('classifyServerMessage', () => {
  it('maps text sentinels to their kinds', () => {
    expect(classifyServerMessage('pong')).toEqual({ kind: 'pong' })
    expect(classifyServerMessage('pty-ready')).toEqual({ kind: 'ready' })
    expect(classifyServerMessage('pty-exited')).toEqual({ kind: 'exited' })
    expect(classifyServerMessage('unprocessable')).toEqual({ kind: 'unprocessable' })
  })

  it('treats any other string as raw PTY data', () => {
    expect(classifyServerMessage('$ ')).toEqual({ kind: 'data', data: '$ ' })
  })

  it('decodes binary frames as bytes', () => {
    const bytes = new Uint8Array([104, 105])
    const msg = classifyServerMessage(bytes.buffer)
    expect(msg.kind).toBe('data')
    expect(msg.kind === 'data' && msg.data).toBeInstanceOf(Uint8Array)
    if (msg.kind === 'data' && msg.data instanceof Uint8Array) {
      expect(Array.from(msg.data)).toEqual([104, 105])
    }
  })
})

describe('session countdown', () => {
  it('counts down from the 8h cap', () => {
    const now = 1_000_000
    expect(remainingSeconds(now, now)).toBe(MAX_SESSION_SECONDS)
    expect(remainingSeconds(now, now + 60_000)).toBe(MAX_SESSION_SECONDS - 60)
    // never negative
    expect(remainingSeconds(now, now + MAX_SESSION_SECONDS * 1000 + 5000)).toBe(0)
  })

  it('bands remaining time into normal / warn / danger', () => {
    expect(sessionLevel(MAX_SESSION_SECONDS)).toBe('normal')
    expect(sessionLevel(1_801)).toBe('normal')
    expect(sessionLevel(1_800)).toBe('warn')
    expect(sessionLevel(301)).toBe('warn')
    expect(sessionLevel(300)).toBe('danger')
    expect(sessionLevel(0)).toBe('danger')
  })

  it('formats remaining time as H:MM:SS / MM:SS', () => {
    expect(formatRemaining(28_800)).toBe('8:00:00')
    expect(formatRemaining(3_661)).toBe('1:01:01')
    expect(formatRemaining(125)).toBe('02:05')
    expect(formatRemaining(-5)).toBe('00:00')
  })
})
