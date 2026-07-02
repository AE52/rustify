import { useState, type ReactNode } from 'react'

export interface WizardStep {
  id: string
  title: string
  /** Gate for leaving this step; the Next button is disabled while false. */
  canAdvance: boolean
  content: ReactNode
}

export interface WizardProps {
  steps: WizardStep[]
  onFinish?: () => void
  finishLabel?: string
  /** Called whenever the active step changes (e.g. to trigger side effects). */
  onStepChange?: (stepId: string) => void
}

/**
 * Linear wizard state machine: steps advance one at a time, never skipping,
 * and a step cannot be left until its `canAdvance` gate is true.
 */
export function Wizard({ steps, onFinish, finishLabel = 'Finish', onStepChange }: WizardProps) {
  const [index, setIndex] = useState(0)
  const step = steps[index]
  const isLast = index === steps.length - 1

  const goTo = (next: number) => {
    setIndex(next)
    onStepChange?.(steps[next].id)
  }

  const next = () => {
    if (!step.canAdvance) return
    if (isLast) {
      onFinish?.()
      return
    }
    goTo(index + 1)
  }

  const back = () => {
    if (index === 0) return
    goTo(index - 1)
  }

  return (
    <div className="flex flex-col gap-8">
      <ol className="flex flex-wrap items-center gap-1 text-xs">
        {steps.map((s, i) => (
          <li key={s.id} className="flex items-center gap-1">
            {i > 0 && <span className="mx-1 text-zinc-700">—</span>}
            <span
              aria-current={i === index ? 'step' : undefined}
              className={
                i === index
                  ? 'rounded-full bg-zinc-100 px-2.5 py-1 font-semibold text-zinc-900'
                  : i < index
                    ? 'px-1 text-emerald-400'
                    : 'px-1 text-zinc-500'
              }
            >
              {s.title}
            </span>
          </li>
        ))}
      </ol>

      <div className="min-h-48">{step.content}</div>

      <div className="flex items-center justify-between border-t border-zinc-800 pt-4">
        <button
          type="button"
          onClick={back}
          disabled={index === 0}
          className="rounded-md border border-zinc-700 px-4 py-1.5 text-sm text-zinc-300 hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-40"
        >
          Back
        </button>
        <button
          type="button"
          onClick={next}
          disabled={!step.canAdvance}
          className="rounded-md bg-zinc-100 px-4 py-1.5 text-sm font-semibold text-zinc-900 hover:bg-white disabled:cursor-not-allowed disabled:opacity-40"
        >
          {isLast ? finishLabel : 'Next'}
        </button>
      </div>
    </div>
  )
}
