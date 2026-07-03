import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import NotificationsPage from './notifications'
import { mockFetch, renderApp } from '../test/harness'

const settings = {
  email_enabled: false,
  smtp_host_configured: false,
  smtp_port: null,
  smtp_encryption: null,
  smtp_username_configured: false,
  smtp_password_configured: false,
  smtp_from_address: null,
  smtp_from_name: null,
  smtp_recipients: null,
  resend_enabled: false,
  resend_api_key_configured: false,
  discord_enabled: false,
  discord_webhook_url_configured: false,
  discord_ping_enabled: false,
  telegram_enabled: false,
  telegram_token_configured: false,
  telegram_chat_id_configured: false,
  slack_enabled: false,
  slack_webhook_url_configured: false,
  pushover_enabled: false,
  pushover_user_key_configured: false,
  pushover_api_token_configured: false,
  webhook_enabled: false,
  webhook_url_configured: false,
  event_matrix: {},
}

function routes(overrides: Record<string, unknown> = {}) {
  return {
    'GET /notifications/settings': settings,
    'PATCH /notifications/settings': (b: unknown) => ({ ...settings, ...(b as object) }),
    'POST /notifications/test': { sent: true, message: 'test notification sent' },
    ...overrides,
  }
}

describe('notifications', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('toggling a channel PATCHes that field', async () => {
    const fetchMock = mockFetch(routes())
    const user = userEvent.setup()
    renderApp(<NotificationsPage />)

    const toggle = await screen.findByLabelText('Discord enabled')
    await user.click(toggle)

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/notifications/settings' && init?.method === 'PATCH',
      )
      expect(call).toBeTruthy()
      const body = JSON.parse(String(call?.[1]?.body))
      expect(body.discord_enabled).toBe(true)
    })
  })

  it('editing the event matrix and saving PATCHes event_matrix', async () => {
    const fetchMock = mockFetch(routes())
    const user = userEvent.setup()
    renderApp(<NotificationsPage />)

    const cell = await screen.findByLabelText('deployment_failure discord')
    await user.click(cell)
    await user.click(screen.getByRole('button', { name: /save event matrix/i }))

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(([url, init]) => {
        if (url !== '/api/v1/notifications/settings' || init?.method !== 'PATCH') return false
        const body = JSON.parse(String(init?.body))
        return body.event_matrix !== undefined
      })
      expect(call).toBeTruthy()
      const body = JSON.parse(String(call?.[1]?.body))
      expect(body.event_matrix.deployment_failure.discord).toBe(true)
    })
  })

  it('the Send test button POSTs the channel', async () => {
    const fetchMock = mockFetch(routes())
    const user = userEvent.setup()
    renderApp(<NotificationsPage />)

    await screen.findByLabelText('Slack enabled')
    // The Slack card's test button is the 4th "Send test" (email, discord, telegram, slack).
    const buttons = screen.getAllByRole('button', { name: /send test/i })
    await user.click(buttons[3])

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([url, init]) => url === '/api/v1/notifications/test' && init?.method === 'POST',
      )
      expect(call).toBeTruthy()
      const body = JSON.parse(String(call?.[1]?.body))
      expect(body.channel).toBe('slack')
    })
  })
})
