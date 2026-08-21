import type { Dims, Dtype, PhysicalScale } from '@cellstudio/api-client'

const DTYPE_LABELS: Record<Dtype, string> = { u8: 'uint8', u16: 'uint16', u32: 'uint32' }

export function formatInt(v: number): string {
  return Math.round(v).toLocaleString('en-US')
}

/** 18420 → "18.4k"; small values stay exact. */
export function formatCompact(v: number): string {
  const abs = Math.abs(v)
  if (abs < 1000) return String(Math.round(v))
  if (abs < 1_000_000) return `${trimZero(v / 1000)}k`
  return `${trimZero(v / 1_000_000)}M`
}

export function formatUm(v: number): string {
  if (v >= 10) return v.toFixed(0)
  if (v >= 1) return v.toFixed(1)
  return String(Number(v.toFixed(3)))
}

export function formatDtype(dtype: Dtype): string {
  return DTYPE_LABELS[dtype]
}

/** Inspector shape line: "400 × 2 × 45 × 2048²". */
export function formatShape(dims: Dims): string {
  const plane = dims.y === dims.x ? `${dims.y}²` : `${dims.y} × ${dims.x}`
  return `${dims.t} × ${dims.c} × ${dims.z} × ${plane}`
}

export function formatVoxelSize(scale: PhysicalScale | null): string {
  if (!scale) return 'unknown'
  return `${formatUm(scale.z)} × ${formatUm(scale.y)} × ${formatUm(scale.x)} µm`
}

export function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean)
  return parts[parts.length - 1] ?? path
}

export function formatPercent(fraction: number): string {
  return `${Math.round(fraction * 100)}%`
}

function trimZero(v: number): string {
  return String(Number(v.toFixed(1)))
}
