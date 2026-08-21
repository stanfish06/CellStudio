import type { Dims } from '@cellstudio/api-client'
import type { sliceAxis } from '@cellstudio/viewer'

/** Axis a slice view steps along, as the nav store's `sliceAxis` reports it. */
export type SliceAxisName = ReturnType<typeof sliceAxis>

export type StepAxis = 't' | SliceAxisName

export interface TransportRow {
  axis: StepAxis
  label: string
  value: number
  /** Inclusive maximum index. */
  max: number
  /** The frame readout is typed into directly; slice readouts are display-only. */
  editable: boolean
}

export interface TransportInput {
  t: number
  /** The active view's slice axis and position, from `sliceAxis`; null in 3D, which has no slice row. */
  slice: { axis: SliceAxisName; index: number } | null
  dims: Dims | null
}

export function transportRows(input: TransportInput): TransportRow[] {
  const rows: TransportRow[] = [
    {
      axis: 't',
      label: 'T',
      value: input.t,
      max: lastIndex(input.dims?.t),
      editable: true,
    },
  ]
  if (!input.slice) return rows

  const { axis, index } = input.slice
  rows.push({
    axis,
    label: axis.toUpperCase(),
    value: index,
    max: lastIndex(input.dims?.[axis]),
    editable: false,
  })
  return rows
}

export function clampIndex(value: number, max: number): number {
  if (!Number.isFinite(value)) return 0
  return Math.min(Math.max(max, 0), Math.max(0, Math.round(value)))
}

/** Text from the editable frame readout; null when it is not a number to jump to. */
export function parseIndex(text: string, max: number): number | null {
  const trimmed = text.trim()
  if (trimmed === '') return null
  const parsed = Number(trimmed)
  if (!Number.isFinite(parsed)) return null
  return clampIndex(parsed, max)
}

/** Slider fill, driven into the `--progress` custom property. */
export function progressPercent(value: number, max: number): string {
  if (max <= 0) return '0%'
  const fraction = Math.min(1, Math.max(0, value / max))
  return `${Math.round(fraction * 1000) / 10}%`
}

/** Playback wraps at the end of the axis rather than stopping. */
export function nextPlaybackIndex(value: number, max: number): number {
  if (max <= 0) return 0
  return value >= max ? 0 : value + 1
}

function lastIndex(size: number | undefined): number {
  return Math.max(0, (size ?? 1) - 1)
}
