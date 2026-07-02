import { describe, expect, it } from 'vitest'
import { ONBOARDING_STEPS, canLeaveStep, type OnboardingState } from './onboarding'

const empty: OnboardingState = {}

describe('onboarding state machine', () => {
  it('follows the pinned step order', () => {
    expect(ONBOARDING_STEPS).toEqual([
      'welcome',
      'key',
      'server',
      'validate',
      'project',
      'app',
      'deploy',
    ])
  })

  it('welcome can always advance', () => {
    expect(canLeaveStep('welcome', empty)).toBe(true)
  })

  it('key requires a created private key', () => {
    expect(canLeaveStep('key', empty)).toBe(false)
    expect(canLeaveStep('key', { privateKeyUuid: 'pk1' })).toBe(true)
  })

  it('server requires a created server', () => {
    expect(canLeaveStep('server', { privateKeyUuid: 'pk1' })).toBe(false)
    expect(canLeaveStep('server', { privateKeyUuid: 'pk1', serverUuid: 's1' })).toBe(true)
  })

  it('validate requires a successful validation', () => {
    const base: OnboardingState = { privateKeyUuid: 'pk1', serverUuid: 's1' }
    expect(canLeaveStep('validate', base)).toBe(false)
    expect(canLeaveStep('validate', { ...base, serverValidated: true })).toBe(true)
  })

  it('project requires a created project', () => {
    const base: OnboardingState = {
      privateKeyUuid: 'pk1',
      serverUuid: 's1',
      serverValidated: true,
    }
    expect(canLeaveStep('project', base)).toBe(false)
    expect(canLeaveStep('project', { ...base, projectUuid: 'p1' })).toBe(true)
  })

  it('app requires a created application', () => {
    const base: OnboardingState = {
      privateKeyUuid: 'pk1',
      serverUuid: 's1',
      serverValidated: true,
      projectUuid: 'p1',
    }
    expect(canLeaveStep('app', base)).toBe(false)
    expect(canLeaveStep('app', { ...base, applicationUuid: 'a1' })).toBe(true)
  })

  it('deploy requires a started deployment', () => {
    const base: OnboardingState = {
      privateKeyUuid: 'pk1',
      serverUuid: 's1',
      serverValidated: true,
      projectUuid: 'p1',
      applicationUuid: 'a1',
    }
    expect(canLeaveStep('deploy', base)).toBe(false)
    expect(canLeaveStep('deploy', { ...base, deploymentUuid: 'd1' })).toBe(true)
  })
})
