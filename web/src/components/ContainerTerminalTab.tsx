import { useState, type FormEvent } from 'react'
import { Terminal } from './Terminal'
import { containerTarget, hostTarget } from '../lib/terminal'
import { btnGhost, btnPrimary, Field, inputCls, SectionTitle } from './ui'

export interface ContainerTerminalTabProps {
  serverUuid: string
  /** Pre-filled container name (usually the resource uuid). */
  defaultContainer?: string
  placeholder?: string
}

/**
 * Interactive shell tab for a resource that runs in a container (database,
 * service). The container name is dynamic per deploy, so the operator confirms
 * it (or clears it for a host shell) before connecting; the server validates
 * the target and refuses when the server's web terminal is disabled.
 */
export function ContainerTerminalTab({
  serverUuid,
  defaultContainer = '',
  placeholder,
}: ContainerTerminalTabProps) {
  const [container, setContainer] = useState(defaultContainer)
  const [target, setTarget] = useState<string | null>(null)

  const connect = (e: FormEvent) => {
    e.preventDefault()
    const name = container.trim()
    setTarget(name ? containerTarget(serverUuid, name) : hostTarget(serverUuid))
  }

  return (
    <div className="flex flex-col gap-4">
      <SectionTitle>Terminal</SectionTitle>
      <form onSubmit={connect} className="flex max-w-2xl items-end gap-3">
        <div className="flex-1">
          <Field label="Container name (leave blank for a host shell)">
            <input
              className={inputCls}
              value={container}
              onChange={(e) => setContainer(e.target.value)}
              placeholder={placeholder}
              spellCheck={false}
            />
          </Field>
        </div>
        <button type="submit" className={target ? btnGhost : btnPrimary}>
          {target ? 'Reconnect' : 'Connect'}
        </button>
      </form>
      <p className="text-xs text-zinc-500">Admins and owners only. Sessions expire after 8 hours.</p>
      {target && <Terminal key={target} target={target} />}
    </div>
  )
}
