import type { LabelScope, LabelState } from '@cellstudio/api-client'
import { useEffect, useRef } from 'react'

export interface LabelPopoverProps {
  scope: LabelScope
  onScope: (scope: LabelScope) => void
  states: readonly LabelState[]
  onToggle: (state: LabelState) => void
  onClose: () => void
}

const SCOPES: readonly { id: LabelScope; label: string; hint: string }[] = [
  { id: 'cell', label: 'Cell', hint: 'Mark the selected cell only' },
  { id: 'track', label: 'Track', hint: 'Mark every cell of the selected track' },
]

/** Anchored under the Assign labels button; a click outside or Escape closes it. */
export function LabelPopover({ scope, onScope, states, onToggle, onClose }: LabelPopoverProps) {
  const root = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Element | null
      // the anchor button toggles on click; closing on its pointerdown would reopen it
      if (target?.closest('[data-label-anchor]')) return
      if (root.current && !root.current.contains(event.target as Node)) onClose()
    }
    document.addEventListener('pointerdown', onPointerDown)
    return () => document.removeEventListener('pointerdown', onPointerDown)
  }, [onClose])

  return (
    <div className="label-popover" ref={root} role="dialog" aria-label="Assign labels">
      <div className="label-scope" role="radiogroup" aria-label="Label scope">
        {SCOPES.map((s) => (
          <button
            key={s.id}
            type="button"
            role="radio"
            aria-checked={s.id === scope}
            className={s.id === scope ? 'label-scope-option active' : 'label-scope-option'}
            title={s.hint}
            onClick={() => onScope(s.id)}
          >
            {s.label}
          </button>
        ))}
      </div>
      {states.length === 0 ? (
        <p className="empty-note">No labels defined. Add one under Inspect › Labels.</p>
      ) : (
        <ul className="label-list">
          {states.map((state) => (
            <LabelRow key={state.name} state={state} scope={scope} onToggle={onToggle} />
          ))}
        </ul>
      )}
    </div>
  )
}

function LabelRow({
  state,
  scope,
  onToggle,
}: {
  state: LabelState
  scope: LabelScope
  onToggle: (state: LabelState) => void
}) {
  const box = useRef<HTMLInputElement>(null)
  const checked = scope === 'cell' ? state.cell : state.track === 'all'
  const partial = scope === 'track' && state.track === 'some'
  useEffect(() => {
    if (box.current) box.current.indeterminate = partial
  }, [partial])
  return (
    <li>
      <label className="label-row" title={partial ? 'On some cells of this track' : undefined}>
        <input
          ref={box}
          type="checkbox"
          checked={checked}
          aria-checked={partial ? 'mixed' : checked}
          onChange={() => onToggle(state)}
        />
        <span className="label-row-name">{state.name}</span>
        {partial ? <span className="label-row-partial">partial</span> : null}
      </label>
    </li>
  )
}
