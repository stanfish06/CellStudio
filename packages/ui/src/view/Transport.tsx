import { sliceAxis, useNav } from '@cellstudio/viewer'
import { useState } from 'react'
import { PauseIcon, PlayIcon } from '../icons'
import { parseIndex, progressPercent, transportRows, type TransportRow } from '../lib/transport'
import { cssVars } from '../types'

export function Transport() {
  const project = useNav((s) => s.project)
  const activeView = useNav((s) => s.activeView)
  const slices = useNav((s) => s.slices)
  const t = useNav((s) => s.t)

  const orientation = activeView === '3d' ? null : activeView
  const axis = orientation ? sliceAxis(orientation) : null
  const rows = transportRows({
    t,
    slice: orientation && axis ? { axis, index: slices[orientation].index } : null,
    dims: project?.dims ?? null,
  })

  return (
    <div className="transport">
      <div className="scrub-stack">
        {rows.map((row) => (
          <ScrubRow key={row.axis} row={row} />
        ))}
      </div>
    </div>
  )
}

function ScrubRow({ row }: { row: TransportRow }) {
  const playing = useNav((s) => s.transport.playing)
  const setPlaying = useNav((s) => s.setPlaying)
  const stepT = useNav((s) => s.stepT)
  const setT = useNav((s) => s.setT)
  const stepSlice = useNav((s) => s.stepSlice)
  const setSliceIndex = useNav((s) => s.setSliceIndex)

  const isTime = row.axis === 't'
  const axisKey = isTime ? 't' : 'slice'
  const step = isTime ? stepT : stepSlice
  const set = isTime ? setT : setSliceIndex
  const noun = isTime ? 'frame' : `${row.label} slice`
  const isPlaying = playing === axisKey

  return (
    <div className="scrub-row">
      <div className="step-controls">
        <button
          type="button"
          className="icon-button"
          aria-label={`Previous ${noun}`}
          onClick={() => step(-1)}
        >
          ‹
        </button>
        <button
          type="button"
          className="icon-button"
          aria-label={isPlaying ? `Pause ${noun}s` : `Play ${noun}s`}
          aria-pressed={isPlaying}
          onClick={() => setPlaying(isPlaying ? 'off' : axisKey)}
        >
          {isPlaying ? <PauseIcon /> : <PlayIcon />}
        </button>
        <button
          type="button"
          className="icon-button"
          aria-label={`Next ${noun}`}
          onClick={() => step(1)}
        >
          ›
        </button>
      </div>
      <span className="axis-chip">{row.label}</span>
      <input
        type="range"
        aria-label={isTime ? 'Time' : `${row.label} slice`}
        min={0}
        max={row.max}
        value={row.value}
        style={cssVars({ '--progress': progressPercent(row.value, row.max) })}
        onChange={(e) => set(Number(e.target.value))}
      />
      <span className="scrub-value">
        {row.editable ? <IndexInput row={row} onCommit={set} /> : row.value} / {row.max}
      </span>
    </div>
  )
}

interface IndexInputProps {
  row: TransportRow
  onCommit: (value: number) => void
}

/** Typing a frame number jumps there; out-of-range entry clamps to the axis. */
function IndexInput({ row, onCommit }: IndexInputProps) {
  const [draft, setDraft] = useState<string | null>(null)

  const commit = () => {
    if (draft === null) return
    const parsed = parseIndex(draft, row.max)
    setDraft(null)
    if (parsed !== null) onCommit(parsed)
  }

  return (
    <input
      type="number"
      aria-label="Current frame"
      min={0}
      max={row.max}
      value={draft ?? row.value}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === 'Enter') commit()
        if (e.key === 'Escape') setDraft(null)
      }}
    />
  )
}
