import type { Dtype, Level } from '@cellstudio/api-client'

export const GPU_BYTES_PER_SAMPLE = 4

export const dtypeBytes = (dtype: Dtype): number => (dtype === 'u8' ? 1 : dtype === 'u16' ? 2 : 4)

export interface GpuBudgetOptions {
  totalBytes?: number
  volumeCeilingBytes?: number
}

export interface VolumePlan {
  level: number
  /** GPU bytes for one timepoint, all visible channels. */
  gpuBytes: number
  /** Wire/CPU bytes for one timepoint, all visible channels. */
  rawBytes: number
  /** Timepoints that fit the volume reservation — 1 means no t±1 prefetch headroom. */
  residentTimepoints: number
  /** True when even the coarsest level exceeds the ceiling. */
  overBudget: boolean
}

/**
 * Sizes the viv tile cache and volume residency against one budget so a slice ↔ 3D
 * switch does not evict what the other view needs.
 */
export class GpuBudget {
  private total: number
  private ceiling: number
  private volumeReserved = 0

  constructor(opts: GpuBudgetOptions = {}) {
    this.total = opts.totalBytes ?? 1024 * 1024 * 1024
    this.ceiling = opts.volumeCeilingBytes ?? 128 * 1024 * 1024
  }

  get totalBytes(): number {
    return this.total
  }

  get volumeCeilingBytes(): number {
    return this.ceiling
  }

  get volumeBytes(): number {
    return this.volumeReserved
  }

  get tileBytes(): number {
    return Math.max(0, this.total - this.volumeReserved)
  }

  setTotal(bytes: number): void {
    this.total = Math.max(0, bytes)
    this.volumeReserved = Math.min(this.volumeReserved, this.total)
  }

  reserveVolume(bytes: number): number {
    this.volumeReserved = Math.min(Math.max(0, bytes), this.total)
    return this.volumeReserved
  }

  releaseVolume(): void {
    this.volumeReserved = 0
  }

  /** Finest pyramid level whose timepoint fits the volume ceiling. */
  planVolume(levels: Level[], channels: number, dtype: Dtype): VolumePlan {
    const chans = Math.max(1, channels)
    const ordered = [...levels].sort((a, b) => a.index - b.index)
    const cost = (level: Level) => {
      const voxels = level.dims.z * level.dims.y * level.dims.x
      return {
        gpuBytes: voxels * GPU_BYTES_PER_SAMPLE * chans,
        rawBytes: voxels * dtypeBytes(dtype) * chans,
      }
    }
    const coarsest = ordered[ordered.length - 1]
    if (!coarsest) {
      return { level: 0, gpuBytes: 0, rawBytes: 0, residentTimepoints: 0, overBudget: true }
    }
    const chosen = ordered.find((level) => cost(level).gpuBytes <= this.ceiling) ?? coarsest
    const { gpuBytes, rawBytes } = cost(chosen)
    const reserved = this.reserveVolume(Math.min(gpuBytes * 2, this.ceiling * 2))
    return {
      level: chosen.index,
      gpuBytes,
      rawBytes,
      residentTimepoints: gpuBytes > 0 ? Math.floor(reserved / gpuBytes) : 0,
      overBudget: gpuBytes > this.ceiling,
    }
  }

  /** Tile count for the multiscale layer's cache, from what the volume left over. */
  tileCacheSize(tileSize: number, channels: number): number {
    const perTile = Math.max(1, tileSize * tileSize * GPU_BYTES_PER_SAMPLE * Math.max(1, channels))
    return Math.min(4096, Math.max(16, Math.floor(this.tileBytes / perTile)))
  }
}

/** Process-wide registry; construct a `GpuBudget` directly for isolated accounting. */
export const gpuBudget = new GpuBudget()
