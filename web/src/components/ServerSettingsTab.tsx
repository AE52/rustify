import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  api,
  type ServerSettings,
  type ServerSettingsUpdate,
  type Team,
} from '../api/client'
import { isAdmin } from '../lib/roles'
import {
  btnDanger,
  btnPrimary,
  cardCls,
  ErrorNote,
  Field,
  inputCls,
  SectionTitle,
  selectCls,
} from './ui'

function Toggle({
  label,
  description,
  checked,
  disabled,
  onChange,
}: {
  label: string
  description: string
  checked: boolean
  disabled: boolean
  onChange: (v: boolean) => void
}) {
  return (
    <label className="flex items-start justify-between gap-4 py-2">
      <span className="flex flex-col">
        <span className="text-sm text-zinc-200">{label}</span>
        <span className="text-xs text-zinc-500">{description}</span>
      </span>
      <input
        type="checkbox"
        role="switch"
        aria-label={label}
        className="mt-1 h-4 w-4 accent-sky-500"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
    </label>
  )
}

/**
 * Server operational settings (§ ops): reverse proxy type, build-server /
 * terminal / metrics toggles (`PATCH /servers/{uuid}/settings`), and the
 * Cloudflare tunnel (`POST`/`DELETE /servers/{uuid}/cloudflared`). Edits are
 * gated on the active team's admin role.
 */
export function ServerSettingsTab({ serverUuid }: { serverUuid: string }) {
  const queryClient = useQueryClient()
  const settings = useQuery({
    queryKey: ['server', serverUuid, 'settings'],
    queryFn: () => api.get<ServerSettings>(`/servers/${serverUuid}/settings`),
  })
  const team = useQuery({
    queryKey: ['team', 'current'],
    queryFn: () => api.get<Team>('/teams/current'),
  })
  const admin = isAdmin(team.data?.role)

  const patch = useMutation({
    mutationFn: (body: ServerSettingsUpdate) =>
      api.patch<ServerSettings>(`/servers/${serverUuid}/settings`, body),
    onSuccess: (data) => queryClient.setQueryData(['server', serverUuid, 'settings'], data),
  })

  // Cloudflare tunnel enable form state.
  const [tunnelToken, setTunnelToken] = useState('')
  const [sshHostname, setSshHostname] = useState('')
  const enableTunnel = useMutation({
    mutationFn: () =>
      api.post(`/servers/${serverUuid}/cloudflared`, {
        tunnel_token: tunnelToken,
        ssh_hostname: sshHostname,
      }),
    onSuccess: () => {
      setTunnelToken('')
      setSshHostname('')
      queryClient.invalidateQueries({ queryKey: ['server', serverUuid, 'settings'] })
    },
  })
  const disableTunnel = useMutation({
    mutationFn: () => api.delete(`/servers/${serverUuid}/cloudflared`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['server', serverUuid, 'settings'] }),
  })

  if (settings.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (settings.isError) return <ErrorNote error={settings.error} />

  const s = settings.data
  const [refresh, disabledEdit] = [s.metrics_refresh_rate_seconds, !admin || patch.isPending]

  return (
    <div className="flex max-w-2xl flex-col gap-6">
      <section className={`${cardCls} flex flex-col gap-3`}>
        <SectionTitle>Proxy</SectionTitle>
        <Field label="Reverse proxy">
          <select
            aria-label="Proxy type"
            className={selectCls}
            value={s.proxy_type}
            disabled={disabledEdit}
            onChange={(e) => patch.mutate({ proxy_type: e.target.value })}
          >
            <option value="traefik">Traefik</option>
            <option value="caddy">Caddy</option>
          </select>
        </Field>
      </section>

      <section className={`${cardCls} flex flex-col divide-y divide-zinc-800`}>
        <SectionTitle>Options</SectionTitle>
        <Toggle
          label="Build server"
          description="Dedicate this server to building images; excluded from deploy targets."
          checked={s.is_build_server}
          disabled={disabledEdit}
          onChange={(v) => patch.mutate({ is_build_server: v })}
        />
        <Toggle
          label="Web terminal"
          description="Allow interactive SSH/container shells from the browser."
          checked={s.is_terminal_enabled}
          disabled={disabledEdit}
          onChange={(v) => patch.mutate({ is_terminal_enabled: v })}
        />
        <Toggle
          label="Metrics"
          description="Collect CPU / memory / disk time series for this server."
          checked={s.metrics_enabled}
          disabled={disabledEdit}
          onChange={(v) => patch.mutate({ metrics_enabled: v })}
        />
        <div className="flex items-center justify-between gap-4 py-2">
          <span className="text-sm text-zinc-200">Metrics refresh (seconds)</span>
          <input
            type="number"
            aria-label="Metrics refresh seconds"
            min={1}
            className={`${inputCls} w-24`}
            defaultValue={refresh}
            disabled={disabledEdit || !s.metrics_enabled}
            onBlur={(e) => {
              const n = Number(e.target.value)
              if (n >= 1 && n !== refresh) patch.mutate({ metrics_refresh_rate_seconds: n })
            }}
          />
        </div>
        <ErrorNote error={patch.error} />
      </section>

      <section className={`${cardCls} flex flex-col gap-3`}>
        <SectionTitle>Cloudflare tunnel</SectionTitle>
        {s.is_cloudflare_tunnel ? (
          <div className="flex items-center justify-between gap-3">
            <span className="text-sm text-emerald-300">Tunnel enabled</span>
            {admin && (
              <button
                type="button"
                className={btnDanger}
                disabled={disableTunnel.isPending}
                onClick={() => disableTunnel.mutate()}
              >
                Disable tunnel
              </button>
            )}
          </div>
        ) : admin ? (
          <form
            onSubmit={(e: FormEvent) => {
              e.preventDefault()
              enableTunnel.mutate()
            }}
            className="flex flex-col gap-3"
          >
            <Field label="Tunnel token">
              <input
                type="password"
                className={inputCls}
                value={tunnelToken}
                onChange={(e) => setTunnelToken(e.target.value)}
                autoComplete="off"
              />
            </Field>
            <Field label="SSH hostname">
              <input
                className={inputCls}
                value={sshHostname}
                onChange={(e) => setSshHostname(e.target.value)}
                placeholder="ssh.example.com"
              />
            </Field>
            <ErrorNote error={enableTunnel.error} />
            <button
              type="submit"
              className={`${btnPrimary} w-fit`}
              disabled={enableTunnel.isPending || !tunnelToken || !sshHostname}
            >
              {enableTunnel.isPending ? 'Enabling…' : 'Enable tunnel'}
            </button>
          </form>
        ) : (
          <p className="text-sm text-zinc-500">Tunnel disabled.</p>
        )}
        {!admin && <p className="text-xs text-zinc-500">Only team admins can change these settings.</p>}
      </section>
    </div>
  )
}
