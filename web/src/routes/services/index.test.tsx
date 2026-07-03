import { afterEach, describe, expect, it, vi } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import ServicesCatalog from './index'
import { mockFetch, renderApp } from '../../test/harness'

const templates = [
  {
    key: 'umami',
    name: 'Umami',
    slogan: 'Privacy-focused analytics',
    documentation: '',
    category: 'Analytics',
    tags: ['analytics'],
    logo: null,
    port: null,
  },
  {
    key: 'ghost',
    name: 'Ghost',
    slogan: 'Publishing platform',
    documentation: '',
    category: 'CMS',
    tags: ['blog', 'cms'],
    logo: null,
    port: null,
  },
]

describe('ServicesCatalog', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('renders a card for each template from the mocked GET', async () => {
    mockFetch({ 'GET /service-templates': templates, 'GET /services': [] })
    renderApp(<ServicesCatalog />)

    await waitFor(() => expect(screen.getByText('Umami')).toBeInTheDocument())
    expect(screen.getByText('Ghost')).toBeInTheDocument()
    expect(screen.getAllByTestId('template-card')).toHaveLength(2)
  })

  it('filters templates by the search box', async () => {
    mockFetch({ 'GET /service-templates': templates, 'GET /services': [] })
    const user = userEvent.setup()
    renderApp(<ServicesCatalog />)

    await waitFor(() => expect(screen.getByText('Umami')).toBeInTheDocument())
    await user.type(screen.getByLabelText('search services'), 'ghost')

    expect(screen.getByText('Ghost')).toBeInTheDocument()
    expect(screen.queryByText('Umami')).not.toBeInTheDocument()
  })
})
