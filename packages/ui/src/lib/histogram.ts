import type { Histogram } from '@cellstudio/api-client'
import { formatCompact } from './format'

export const HIST_WIDTH = 284
export const HIST_HEIGHT = 74
/** Keeps the transfer curve's endpoints off the plot border, as in the prototype. */
const CURVE_INSET = 2
const CURVE_SAMPLES = 24

export interface HistogramGeometry {
  width: number
  height: number
  /** Closed area under the distribution; empty when no counts are loaded. */
  fill: string
  outline: string
  /** Shaded display window. */
  window: { x: number; width: number }
  minX: number
  maxX: number
  /** LUT transfer curve across the window. */
  curve: string
  ticks: [string, string, string]
  domain: [number, number]
}

/** Normalized intensity → normalized display. gamma < 1 brightens (napari convention). */
export function lutTransfer(t: number, gamma: number): number {
  const clamped = Math.min(1, Math.max(0, t))
  return Math.pow(clamped, gamma)
}

export interface HistogramInput {
  hist: Histogram | null
  /** Full value range of the channel's dtype, used until a histogram arrives. */
  domain: readonly [number, number]
  window: readonly [number, number]
  gamma: number
  size?: { width: number; height: number }
}

export function histogramGeometry(input: HistogramInput): HistogramGeometry {
  const width = input.size?.width ?? HIST_WIDTH
  const height = input.size?.height ?? HIST_HEIGHT
  const hist = input.hist
  const lo = hist ? hist.min : input.domain[0]
  const hi = hist ? hist.max : input.domain[1]
  const span = hi - lo || 1

  const xOf = (v: number) => Math.min(width, Math.max(0, ((v - lo) / span) * width))

  const minX = xOf(input.window[0])
  const maxX = xOf(input.window[1])
  const left = Math.min(minX, maxX)
  const right = Math.max(minX, maxX)

  return {
    width,
    height,
    ...distributionPaths(hist, width, height),
    window: { x: left, width: Math.max(1, right - left) },
    minX,
    maxX,
    curve: transferPath(minX, maxX, input.gamma, height),
    ticks: [formatCompact(lo), formatCompact(lo + span / 2), formatCompact(hi)],
    domain: [lo, hi],
  }
}

function distributionPaths(
  hist: Histogram | null,
  width: number,
  height: number,
): { fill: string; outline: string } {
  const counts = hist?.counts ?? []
  const peak = counts.reduce((m, c) => Math.max(m, c), 0)
  if (counts.length === 0 || peak <= 0) return { fill: '', outline: '' }

  const step = width / Math.max(1, counts.length - 1)
  const points = counts.map((c, i) => {
    const x = round2(i * step)
    const y = round2(height - (c / peak) * height)
    return `${x} ${y}`
  })
  const outline = `M${points.join('L')}`
  return { fill: `M0 ${height}L${points.join('L')}L${round2(width)} ${height}Z`, outline }
}

// Sampled rather than a straight line so gamma is visible in the curve itself.
function transferPath(minX: number, maxX: number, gamma: number, height: number): string {
  const top = CURVE_INSET
  const bottom = height - CURVE_INSET
  const points: string[] = []
  for (let i = 0; i <= CURVE_SAMPLES; i++) {
    const t = i / CURVE_SAMPLES
    const x = round2(minX + (maxX - minX) * t)
    const y = round2(bottom - lutTransfer(t, gamma) * (bottom - top))
    points.push(`${x} ${y}`)
  }
  return `M${points.join('L')}`
}

function round2(v: number): number {
  return Math.round(v * 100) / 100
}
