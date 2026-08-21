import type { PhysicalScale } from '@cellstudio/api-client'
import type { ActiveView } from '@cellstudio/viewer'
import { formatUm } from './format'
import { VIEW_LABELS } from './keymap'
import type { SliceAxisName } from './transport'

export interface Chip {
  prefix: string
  emphasis: string
  suffix: string
}

export const chipText = (chip: Chip): string => `${chip.prefix}${chip.emphasis}${chip.suffix}`

export interface SliceChip {
  axis: SliceAxisName
  index: number
  max: number
}

export interface HudInput {
  activeView: ActiveView
  slice: SliceChip | null
  t: number
  tMax: number
  level: number
  zoom: number
  scale: PhysicalScale | null
}

export interface HudChips {
  orientation: Chip
  level: string
  frame: Chip
  voxel: string
}

export function hudChips(input: HudInput): HudChips {
  return {
    orientation: orientationChip(input.activeView, input.slice),
    level: levelChip(input.level, input.zoom),
    frame: frameChip(input.t, input.tMax),
    voxel: voxelChip(input.scale),
  }
}

export function orientationChip(view: ActiveView, slice: SliceChip | null): Chip {
  return {
    prefix: '',
    emphasis: VIEW_LABELS[view],
    suffix: slice ? ` · ${slice.axis.toUpperCase()} ${slice.index}/${slice.max}` : '',
  }
}

export function frameChip(t: number, tMax: number): Chip {
  return { prefix: 'T ', emphasis: String(t), suffix: `/${tMax}` }
}

export function levelChip(level: number, zoom: number): string {
  return `Scale ${level} · ${zoomPercent(zoom)}`
}

export function zoomPercent(zoom: number): string {
  const percent = Math.pow(2, zoom) * 100
  return `${percent >= 100 ? Math.round(percent) : Math.round(percent * 10) / 10}%`
}

export function voxelChip(scale: PhysicalScale | null): string {
  if (!scale) return 'voxel size unknown'
  if (scale.y === scale.x) return `${formatUm(scale.z)} µm Z · ${formatUm(scale.x)} µm XY`
  return `${formatUm(scale.z)} Z · ${formatUm(scale.y)} Y · ${formatUm(scale.x)} X µm`
}

export interface ScaleBar {
  lengthPx: number
  label: string
}

/**
 * Screen pixels per µm along the in-plane axis, from the level-0 voxel size and camera zoom.
 * Null when the dataset has no physical scale.
 */
export function pixelsPerUm(scale: PhysicalScale | null, zoom: number): number | null {
  if (!scale || scale.x <= 0) return null
  return Math.pow(2, zoom) / scale.x
}

/** Nearest 1/2/5 × 10ⁿ µm whose on-screen length is closest to `targetPx`. */
export function scaleBar(pxPerUm: number | null, targetPx = 82): ScaleBar | null {
  if (pxPerUm === null || !Number.isFinite(pxPerUm) || pxPerUm <= 0) return null
  const raw = targetPx / pxPerUm
  const exponent = Math.floor(Math.log10(raw))
  const decade = Math.pow(10, exponent)
  const mantissa = raw / decade
  const nice = mantissa >= 5 ? 5 : mantissa >= 2 ? 2 : 1
  const length = nice * decade
  return { lengthPx: Math.round(length * pxPerUm), label: `${formatUm(length)} µm` }
}
