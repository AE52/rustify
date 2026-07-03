import { describe, expect, it } from 'vitest'
import { buildManifest, manifestAction, parseBranch, parseRepo } from './github'

describe('buildManifest', () => {
  it('includes pull_request permission/event only when preview deployments enabled', () => {
    const withPreview = buildManifest({ name: 'app', baseUrl: 'https://ci.example.com/', previewDeployments: true })
    expect(withPreview.default_permissions.pull_requests).toBe('write')
    expect(withPreview.default_events).toContain('pull_request')
    expect(withPreview.redirect_url).toBe('https://ci.example.com/webhooks/source/github/redirect')
    expect(withPreview.hook_attributes.url).toBe('https://ci.example.com/webhooks/source/github/events')

    const noPreview = buildManifest({ name: 'app', baseUrl: 'https://ci.example.com', previewDeployments: false })
    expect(noPreview.default_permissions.pull_requests).toBeUndefined()
    expect(noPreview.default_events).not.toContain('pull_request')
  })
})

describe('manifestAction', () => {
  it('uses org-scoped path when an organization is given', () => {
    expect(manifestAction('https://github.com', 'ST8', 'acme')).toBe(
      'https://github.com/organizations/acme/settings/apps/new?state=ST8',
    )
  })
  it('uses personal path when no organization', () => {
    expect(manifestAction('https://github.com/', 'ST8', null)).toBe(
      'https://github.com/settings/apps/new?state=ST8',
    )
  })
})

describe('parseRepo / parseBranch', () => {
  it('normalizes a raw installation repo', () => {
    const r = parseRepo({
      id: 5,
      name: 'web',
      full_name: 'acme/web',
      owner: { login: 'acme' },
      private: true,
      default_branch: 'develop',
    })
    expect(r).toEqual({
      id: 5,
      name: 'web',
      full_name: 'acme/web',
      owner: 'acme',
      private: true,
      default_branch: 'develop',
    })
  })
  it('returns null for junk', () => {
    expect(parseRepo(null)).toBeNull()
    expect(parseRepo({})).toBeNull()
    expect(parseBranch({ nope: 1 })).toBeNull()
    expect(parseBranch({ name: 'main' })).toBe('main')
  })
})
