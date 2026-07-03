import { useState, type ReactNode } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  api,
  type NotificationSettings,
  type NotificationSettingsUpdate,
  type NotificationTestResponse,
} from '../api/client'
import {
  CHANNEL_LABELS,
  matrixCell,
  NOTIFY_CHANNELS,
  NOTIFY_EVENTS,
  setMatrixCell,
  type EventMatrix,
  type NotifyChannel,
} from '../lib/notify'
import {
  btnGhost,
  btnPrimary,
  cardCls,
  ErrorNote,
  Field,
  inputCls,
  PageTitle,
  SectionTitle,
} from '../components/ui'

function useSettings() {
  return useQuery({
    queryKey: ['notification-settings'],
    queryFn: () => api.get<NotificationSettings>('/notifications/settings'),
  })
}

function useSave() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (patch: NotificationSettingsUpdate) =>
      api.patch<NotificationSettings>('/notifications/settings', patch),
    onSuccess: (data) => queryClient.setQueryData(['notification-settings'], data),
  })
}

/** A channel card: enable toggle, config fields, write-only secrets, send test. */
function ChannelCard({
  channel,
  title,
  enabled,
  onToggle,
  children,
}: {
  channel: NotifyChannel
  title: string
  enabled: boolean
  onToggle: (v: boolean) => void
  children?: ReactNode
}) {
  const [result, setResult] = useState<NotificationTestResponse | null>(null)
  const test = useMutation({
    mutationFn: () =>
      api.post<NotificationTestResponse>('/notifications/test', { channel }),
    onSuccess: (r) => setResult(r),
  })

  return (
    <div className={`${cardCls} flex flex-col gap-3`}>
      <div className="flex items-center gap-3">
        <label className="flex items-center gap-2 text-sm font-semibold text-zinc-200">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => onToggle(e.target.checked)}
            className="accent-zinc-400"
            aria-label={`${title} enabled`}
          />
          {title}
        </label>
        <button
          type="button"
          className={`${btnGhost} ml-auto py-1 text-xs`}
          disabled={test.isPending}
          onClick={() => test.mutate()}
        >
          {test.isPending ? 'Sending…' : 'Send test'}
        </button>
      </div>
      {children}
      {result && (
        <p className={`text-xs ${result.sent ? 'text-emerald-400' : 'text-red-400'}`}>{result.message}</p>
      )}
      <ErrorNote error={test.error} />
    </div>
  )
}

/** A write-only secret input showing "configured" state when set server-side. */
function SecretField({
  label,
  value,
  configured,
  onChange,
}: {
  label: string
  value: string
  configured: boolean
  onChange: (v: string) => void
}) {
  return (
    <Field label={`${label}${configured ? ' (configured — leave blank to keep)' : ''}`}>
      <input
        type="password"
        className={`${inputCls} font-mono`}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={configured ? '••••••••' : ''}
        autoComplete="off"
      />
    </Field>
  )
}

function ChannelsSection({ s }: { s: NotificationSettings }) {
  const save = useSave()
  // Local draft for secret/config inputs; toggles save immediately.
  const [smtpHost, setSmtpHost] = useState('')
  const [smtpPassword, setSmtpPassword] = useState('')
  const [resendKey, setResendKey] = useState('')
  const [discordUrl, setDiscordUrl] = useState('')
  const [telegramToken, setTelegramToken] = useState('')
  const [telegramChat, setTelegramChat] = useState('')
  const [slackUrl, setSlackUrl] = useState('')
  const [pushoverUser, setPushoverUser] = useState('')
  const [pushoverToken, setPushoverToken] = useState('')
  const [webhookUrl, setWebhookUrl] = useState('')

  const toggle = (patch: NotificationSettingsUpdate) => save.mutate(patch)

  return (
    <section className="flex flex-col gap-4">
      <SectionTitle>Channels</SectionTitle>
      <ErrorNote error={save.error} />

      <ChannelCard
        channel="email"
        title="Email (SMTP + Resend)"
        enabled={s.email_enabled}
        onToggle={(v) => toggle({ email_enabled: v })}
      >
        <Field label="SMTP host">
          <input
            className={`${inputCls} font-mono`}
            value={smtpHost}
            onChange={(e) => setSmtpHost(e.target.value)}
            placeholder={s.smtp_host_configured ? 'configured' : 'smtp.example.com'}
          />
        </Field>
        <SecretField
          label="SMTP password"
          value={smtpPassword}
          configured={s.smtp_password_configured}
          onChange={setSmtpPassword}
        />
        <label className="flex items-center gap-2 text-sm text-zinc-300">
          <input
            type="checkbox"
            checked={s.resend_enabled}
            onChange={(e) => toggle({ resend_enabled: e.target.checked })}
            className="accent-zinc-400"
            aria-label="Resend enabled"
          />
          Use Resend API
        </label>
        <SecretField
          label="Resend API key"
          value={resendKey}
          configured={s.resend_api_key_configured}
          onChange={setResendKey}
        />
        <button
          type="button"
          className={`${btnPrimary} w-fit`}
          disabled={save.isPending}
          onClick={() =>
            save.mutate({
              smtp_host: smtpHost || null,
              smtp_password: smtpPassword || null,
              resend_api_key: resendKey || null,
            })
          }
        >
          Save email
        </button>
      </ChannelCard>

      <ChannelCard
        channel="discord"
        title="Discord"
        enabled={s.discord_enabled}
        onToggle={(v) => toggle({ discord_enabled: v })}
      >
        <SecretField
          label="Webhook URL"
          value={discordUrl}
          configured={s.discord_webhook_url_configured}
          onChange={setDiscordUrl}
        />
        <label className="flex items-center gap-2 text-sm text-zinc-300">
          <input
            type="checkbox"
            checked={s.discord_ping_enabled}
            onChange={(e) => toggle({ discord_ping_enabled: e.target.checked })}
            className="accent-zinc-400"
            aria-label="Discord ping enabled"
          />
          Ping @everyone
        </label>
        <button
          type="button"
          className={`${btnPrimary} w-fit`}
          disabled={save.isPending}
          onClick={() => save.mutate({ discord_webhook_url: discordUrl || null })}
        >
          Save Discord
        </button>
      </ChannelCard>

      <ChannelCard
        channel="telegram"
        title="Telegram"
        enabled={s.telegram_enabled}
        onToggle={(v) => toggle({ telegram_enabled: v })}
      >
        <SecretField
          label="Bot token"
          value={telegramToken}
          configured={s.telegram_token_configured}
          onChange={setTelegramToken}
        />
        <SecretField
          label="Chat ID"
          value={telegramChat}
          configured={s.telegram_chat_id_configured}
          onChange={setTelegramChat}
        />
        <button
          type="button"
          className={`${btnPrimary} w-fit`}
          disabled={save.isPending}
          onClick={() =>
            save.mutate({ telegram_token: telegramToken || null, telegram_chat_id: telegramChat || null })
          }
        >
          Save Telegram
        </button>
      </ChannelCard>

      <ChannelCard
        channel="slack"
        title="Slack"
        enabled={s.slack_enabled}
        onToggle={(v) => toggle({ slack_enabled: v })}
      >
        <SecretField
          label="Webhook URL"
          value={slackUrl}
          configured={s.slack_webhook_url_configured}
          onChange={setSlackUrl}
        />
        <button
          type="button"
          className={`${btnPrimary} w-fit`}
          disabled={save.isPending}
          onClick={() => save.mutate({ slack_webhook_url: slackUrl || null })}
        >
          Save Slack
        </button>
      </ChannelCard>

      <ChannelCard
        channel="pushover"
        title="Pushover"
        enabled={s.pushover_enabled}
        onToggle={(v) => toggle({ pushover_enabled: v })}
      >
        <SecretField
          label="User key"
          value={pushoverUser}
          configured={s.pushover_user_key_configured}
          onChange={setPushoverUser}
        />
        <SecretField
          label="API token"
          value={pushoverToken}
          configured={s.pushover_api_token_configured}
          onChange={setPushoverToken}
        />
        <button
          type="button"
          className={`${btnPrimary} w-fit`}
          disabled={save.isPending}
          onClick={() =>
            save.mutate({ pushover_user_key: pushoverUser || null, pushover_api_token: pushoverToken || null })
          }
        >
          Save Pushover
        </button>
      </ChannelCard>

      <ChannelCard
        channel="webhook"
        title="Webhook"
        enabled={s.webhook_enabled}
        onToggle={(v) => toggle({ webhook_enabled: v })}
      >
        <SecretField
          label="Webhook URL"
          value={webhookUrl}
          configured={s.webhook_url_configured}
          onChange={setWebhookUrl}
        />
        <button
          type="button"
          className={`${btnPrimary} w-fit`}
          disabled={save.isPending}
          onClick={() => save.mutate({ webhook_url: webhookUrl || null })}
        >
          Save Webhook
        </button>
      </ChannelCard>
    </section>
  )
}

function EventMatrixSection({ s }: { s: NotificationSettings }) {
  const save = useSave()
  const initial = (s.event_matrix ?? {}) as EventMatrix
  const [matrix, setMatrix] = useState<EventMatrix>(initial)

  const toggle = (event: string, channel: string, value: boolean) =>
    setMatrix((m) => setMatrixCell(m, event, channel, value))

  return (
    <section className="flex flex-col gap-3">
      <SectionTitle>Event matrix</SectionTitle>
      <div className="overflow-x-auto rounded-lg border border-zinc-800">
        <table className="w-full border-collapse text-sm">
          <thead>
            <tr className="border-b border-zinc-800 text-left text-xs text-zinc-500">
              <th className="px-3 py-2 font-medium">Event</th>
              {NOTIFY_CHANNELS.map((c) => (
                <th key={c} className="px-3 py-2 text-center font-medium">
                  {CHANNEL_LABELS[c]}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {NOTIFY_EVENTS.map((ev) => (
              <tr key={ev.slug} className="border-b border-zinc-900">
                <td className="px-3 py-1.5 text-zinc-300">{ev.label}</td>
                {NOTIFY_CHANNELS.map((c) => (
                  <td key={c} className="px-3 py-1.5 text-center">
                    <input
                      type="checkbox"
                      className="accent-zinc-400"
                      aria-label={`${ev.slug} ${c}`}
                      checked={matrixCell(matrix, ev.slug, c)}
                      onChange={(e) => toggle(ev.slug, c, e.target.checked)}
                    />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <ErrorNote error={save.error} />
      <button
        type="button"
        className={`${btnPrimary} w-fit`}
        disabled={save.isPending}
        onClick={() => save.mutate({ event_matrix: matrix })}
      >
        {save.isPending ? 'Saving…' : 'Save event matrix'}
      </button>
    </section>
  )
}

export default function NotificationsPage() {
  const settings = useSettings()

  if (settings.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (settings.isError) return <ErrorNote error={settings.error} />

  return (
    <div className="flex flex-col gap-8">
      <PageTitle>Notifications</PageTitle>
      <ChannelsSection s={settings.data} />
      <EventMatrixSection s={settings.data} />
    </div>
  )
}
