import type { PhysicalScale } from '@cellstudio/api-client'
import type { PixelZYX } from '../data/world'

/** The axis a slice view pins, so a 2D stamp is one voxel thick along the view normal. */
export type StampAxis = 'z' | 'y' | 'x'

export interface StampPlane {
  axis: StampAxis
  /** Exact level-0 index on `axis`. */
  index: number
}

/** A contiguous x span at one `(z, y)`; `x1` is inclusive. */
export interface VoxelRun {
  z: number
  y: number
  x0: number
  x1: number
}

/** Inclusive level-0 voxel box. */
export interface VoxelBox {
  z0: number
  z1: number
  y0: number
  y1: number
  x0: number
  x1: number
}

/** A level-0 voxel set, held as x runs sorted by `(z, y, x0)` and never overlapping. */
export interface VoxelSet {
  readonly runs: readonly VoxelRun[]
}

/** Slot each pinned axis occupies in a `[z, y, x]` coordinate. */
export const AXIS_SLOT: Record<StampAxis, 0 | 1 | 2> = { z: 0, y: 1, x: 2 }

/** Sorts and merges, so a set built from overlapping stamps holds each voxel once. */
export function voxelSet(runs: VoxelRun[]): VoxelSet {
  const kept = runs.filter((r) => r.x0 <= r.x1)
  kept.sort((a, b) => a.z - b.z || a.y - b.y || a.x0 - b.x0 || a.x1 - b.x1)
  const merged: VoxelRun[] = []
  for (const run of kept) {
    const last = merged[merged.length - 1]
    if (last && last.z === run.z && last.y === run.y && run.x0 <= last.x1 + 1) {
      last.x1 = Math.max(last.x1, run.x1)
    } else {
      merged.push({ ...run })
    }
  }
  return { runs: merged }
}

export function voxelCount(set: VoxelSet): number {
  let total = 0
  for (const run of set.runs) total += run.x1 - run.x0 + 1
  return total
}

/** Sorted by `(z, y, x)`. */
export function* voxels(set: VoxelSet): Generator<PixelZYX> {
  for (const run of set.runs) {
    for (let x = run.x0; x <= run.x1; x++) yield [run.z, run.y, x]
  }
}

export function voxelBounds(set: VoxelSet): VoxelBox | null {
  const first = set.runs[0]
  if (!first) return null
  const b: VoxelBox = {
    z0: first.z,
    z1: first.z,
    y0: first.y,
    y1: first.y,
    x0: first.x0,
    x1: first.x1,
  }
  for (const run of set.runs) {
    b.z0 = Math.min(b.z0, run.z)
    b.z1 = Math.max(b.z1, run.z)
    b.y0 = Math.min(b.y0, run.y)
    b.y1 = Math.max(b.y1, run.y)
    b.x0 = Math.min(b.x0, run.x0)
    b.x1 = Math.max(b.x1, run.x1)
  }
  return b
}

export const unionVoxelSets = (sets: readonly VoxelSet[]): VoxelSet =>
  voxelSet(sets.flatMap((s) => s.runs.map((r) => ({ ...r }))))

/** Per-axis voxel radii `[rz, ry, rx]` for a radius stated in level-0 x pixels. */
export function stampRadii(r: number, scale: PhysicalScale | null): [number, number, number] {
  const usable =
    scale !== null &&
    scale.z > 0 &&
    scale.y > 0 &&
    scale.x > 0 &&
    Number.isFinite(scale.z) &&
    Number.isFinite(scale.y) &&
    Number.isFinite(scale.x)
  const s = usable ? (scale as PhysicalScale) : { z: 1, y: 1, x: 1 }
  return [(r * s.x) / s.z, (r * s.x) / s.y, r]
}

/**
 * The level-0 rasterization contract, byte-for-byte the Rust `stamp_voxels`:
 * `centre` in fractional level-0 voxel coordinates, membership by voxel centre (voxel
 * `i` spans `[i, i+1)`), inclusive bounds, clipped to `dims`, and `plane` pinning one
 * axis to one exact index so the 2D disk is the ellipsoid intersected with that slice.
 * Held to the same cases as `crates/cellstudio-core/tests/labels.rs` (design M5).
 */
export function stampVoxels(
  centre: PixelZYX,
  r: number,
  scale: PhysicalScale | null,
  plane: StampPlane | null,
  dims: PixelZYX,
): VoxelSet {
  if (!Number.isFinite(r) || r <= 0 || dims.some((d) => d <= 0)) return { runs: [] }
  const rad = stampRadii(r, scale)
  const lo: [number, number, number] = [0, 0, 0]
  const hi: [number, number, number] = [0, 0, 0]
  for (let i = 0; i < 3; i++) {
    const a = Math.floor(centre[i]! - rad[i]! - 0.5)
    const b = Math.floor(centre[i]! + rad[i]!)
    if (b < 0 || a >= dims[i]!) return { runs: [] }
    lo[i] = Math.max(a, 0)
    hi[i] = Math.min(b, dims[i]! - 1)
  }
  if (plane) {
    const slot = AXIS_SLOT[plane.axis]
    if (plane.index < lo[slot] || plane.index > hi[slot]) return { runs: [] }
    lo[slot] = plane.index
    hi[slot] = plane.index
  }

  const runs: VoxelRun[] = []
  for (let z = lo[0]; z <= hi[0]; z++) {
    const dz = (z + 0.5 - centre[0]) / rad[0]
    for (let y = lo[1]; y <= hi[1]; y++) {
      const dy = (y + 0.5 - centre[1]) / rad[1]
      let open: VoxelRun | null = null
      for (let x = lo[2]; x <= hi[2]; x++) {
        const dx = (x + 0.5 - centre[2]) / rad[2]
        if (dz * dz + dy * dy + dx * dx <= 1) {
          if (open && open.x1 + 1 === x) open.x1 = x
          else {
            if (open) runs.push(open)
            open = { z, y, x0: x, x1: x }
          }
        } else if (open) {
          runs.push(open)
          open = null
        }
      }
      if (open) runs.push(open)
    }
  }
  return voxelSet(runs)
}

/**
 * The only way a coarse voxel set is produced: a coarse voxel belongs to the set when the
 * level-0 voxel it point-samples does. Nothing re-rasterizes an ellipsoid at another
 * level, so the echo matches the store at every level.
 */
export function downsample(set: VoxelSet, factor: PixelZYX): VoxelSet {
  const f: [number, number, number] = [
    Math.max(factor[0], 1),
    Math.max(factor[1], 1),
    Math.max(factor[2], 1),
  ]
  const runs: VoxelRun[] = []
  for (const run of set.runs) {
    if (run.z % f[0] !== 0 || run.y % f[1] !== 0) continue
    const x0 = Math.ceil(run.x0 / f[2])
    const x1 = Math.floor(run.x1 / f[2])
    if (x0 > x1) continue
    runs.push({ z: run.z / f[0], y: run.y / f[1], x0, x1 })
  }
  return voxelSet(runs)
}

/**
 * FNV-1a over each voxel's z, y, x as little-endian u32, in `(z, y, x)` order — the
 * digest the shared fixture records for sets too large to list.
 */
export function stampHash(set: VoxelSet): number {
  let h = 0x811c9dc5
  for (const voxel of voxels(set)) {
    for (const value of voxel) {
      for (const shift of [0, 8, 16, 24]) {
        h = (h ^ ((value >>> shift) & 0xff)) >>> 0
        h = Math.imul(h, 0x01000193) >>> 0
      }
    }
  }
  return h >>> 0
}
