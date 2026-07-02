import { describe, expect, it, vi } from 'vitest'
import { useState } from 'react'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { Wizard } from './Wizard'

function Harness({ onFinish }: { onFinish?: () => void }) {
  const [name, setName] = useState('')
  return (
    <Wizard
      onFinish={onFinish}
      steps={[
        { id: 'welcome', title: 'Welcome', canAdvance: true, content: <p>hello there</p> },
        {
          id: 'name',
          title: 'Name',
          canAdvance: name.trim().length > 0,
          content: (
            <input
              aria-label="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          ),
        },
        { id: 'done', title: 'Done', canAdvance: true, content: <p>all done</p> },
      ]}
    />
  )
}

describe('Wizard', () => {
  it('renders the first step and advances', async () => {
    const user = userEvent.setup()
    render(<Harness />)

    expect(screen.getByText('hello there')).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: /next/i }))
    expect(screen.getByLabelText('name')).toBeInTheDocument()
  })

  it('cannot advance when required fields are missing', async () => {
    const user = userEvent.setup()
    render(<Harness />)
    await user.click(screen.getByRole('button', { name: /next/i }))

    const next = screen.getByRole('button', { name: /next/i })
    expect(next).toBeDisabled()
    await user.click(next)
    // still on the name step
    expect(screen.getByLabelText('name')).toBeInTheDocument()
    expect(screen.queryByText('all done')).not.toBeInTheDocument()
  })

  it('advances once required fields are filled', async () => {
    const user = userEvent.setup()
    render(<Harness />)
    await user.click(screen.getByRole('button', { name: /next/i }))

    await user.type(screen.getByLabelText('name'), 'prod-1')
    const next = screen.getByRole('button', { name: /next/i })
    expect(next).toBeEnabled()
    await user.click(next)
    expect(screen.getByText('all done')).toBeInTheDocument()
  })

  it('goes back to the previous step', async () => {
    const user = userEvent.setup()
    render(<Harness />)
    await user.click(screen.getByRole('button', { name: /next/i }))

    await user.click(screen.getByRole('button', { name: /back/i }))
    expect(screen.getByText('hello there')).toBeInTheDocument()
  })

  it('marks the active step and finishes on the last step', async () => {
    const user = userEvent.setup()
    const onFinish = vi.fn()
    render(<Harness onFinish={onFinish} />)

    expect(screen.getByText('Welcome')).toHaveAttribute('aria-current', 'step')
    await user.click(screen.getByRole('button', { name: /next/i }))
    await user.type(screen.getByLabelText('name'), 'prod-1')
    await user.click(screen.getByRole('button', { name: /next/i }))

    expect(screen.getByText('Done')).toHaveAttribute('aria-current', 'step')
    await user.click(screen.getByRole('button', { name: /finish/i }))
    expect(onFinish).toHaveBeenCalledTimes(1)
  })
})
