import { useState, type FormEvent } from 'react'
import { useApplication } from './index'
import { Terminal } from '../../../components/Terminal'
import { containerTarget, hostTarget } from '../../../lib/terminal'
import { btnGhost, btnPrimary, Field, inputCls, SectionTitle } from '../../../components/ui'

/**
 * Application terminal tab: an interactive shell inside the app's running
 * container (or on its host). The container name is dynamic per deploy
 * (`<uuid>-<shortid>`), so the operator confirms/pastes it before connecting;
 * the server validates that the name is a running container before spawning.
 */
export default function ApplicationTerminal() {
  const { app } = useApplication()
  const [container, setContainer] = useState('')
  const [target, setTarget] = useState<string | null>(null)

  const connect = (e: FormEvent) => {
    e.preventDefault()
    const name = container.trim()
    setTarget(name ? containerTarget(app.server_uuid, name) : hostTarget(app.server_uuid))
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
              placeholder={`${app.uuid}-…`}
              spellCheck={false}
            />
          </Field>
        </div>
        <button type="submit" className={target ? btnGhost : btnPrimary}>
          {target ? 'Reconnect' : 'Connect'}
        </button>
      </form>
      <p className="text-xs text-zinc-500">
        Admins and owners only. Sessions expire after 8 hours.
      </p>
      {target && <Terminal key={target} target={target} />}
    </div>
  )
}
