import type { AxisScale } from '@cellstudio/viewer'

/** Mirrors the nav store's AXIS_SCALE_MIN/MAX; call sites pass the store's constants. */
export const DEFAULT_AXIS_SCALE_MIN = 0.1
export const DEFAULT_AXIS_SCALE_MAX = 10

export const AXIS_SCALE_KEYS: readonly (keyof AxisScale)[] = ['z', 'y', 'x']

export function clampAxisScale(
  value: number,
  min = DEFAULT_AXIS_SCALE_MIN,
  max = DEFAULT_AXIS_SCALE_MAX,
): number {
  if (!Number.isFinite(value)) return 1
  return Math.min(max, Math.max(min, value))
}

/** Numeric entry from the display-scale fields; null when the text is not a multiplier. */
export function parseAxisScale(
  text: string,
  min = DEFAULT_AXIS_SCALE_MIN,
  max = DEFAULT_AXIS_SCALE_MAX,
): number | null {
  const trimmed = text.trim().replace(/×$/, '').trim()
  if (trimmed === '') return null
  const parsed = Number(trimmed)
  if (!Number.isFinite(parsed)) return null
  return clampAxisScale(parsed, min, max)
}

/** True when display scaling is off — every axis at pure physical aspect. */
export function isPhysicalScale(scale: AxisScale): boolean {
  return AXIS_SCALE_KEYS.every((axis) => scale[axis] === 1)
}

export function formatAxisScale(value: number): string {
  return `${Number(value.toFixed(2))}×`
}
