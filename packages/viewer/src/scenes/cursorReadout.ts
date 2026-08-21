import type { PixelApi } from '../data/api'
import { pixelFromSliceWorld, type PixelZYX, type WorldTransform } from '../data/world'
import type { SliceOrientation } from '../state/nav'
import { Emitter } from './types'

/**
 * Matches `CursorSample` in `@cellstudio/ui`, which renders it in the status bar.
 * Coordinates are dataset pixels; display scaling divides out before this.
 */
export interface CursorSample {
  z: number
  y: number
  x: number
  /** Active-channel intensity; null while the lookup is in flight or it failed. */
  value: number | null
  /** Label voxel value, which is the cell id; null without a label layer. */
  labelId: number | null
  trackId: number | null
}

/** Cell rows the overlay window has loaded; `TrackSource` satisfies it. */
export interface TrackLookup {
  trackIdFor(cellId: number): number | null
}

export interface CursorContext {
  t: number
  channel: number
  /** Read the label layer at the same pixel; set when label masks are loaded. */
  labels?: boolean
}

/**
 * Floor between lookups. The 100 ms readout budget is this plus one `/pixel` round trip
 * against the server's warm brick cache.
 */
export const READOUT_THROTTLE_MS = 50

export interface CursorReadoutOptions {
  api: PixelApi
  throttleMs?: number
  tracks?: TrackLookup
  now?: () => number
}

interface Target {
  pixel: PixelZYX
  t: number
  channel: number
  labels: boolean
}

const at = (pixel: PixelZYX): Pick<CursorSample, 'z' | 'y' | 'x'> => ({
  z: pixel[0],
  y: pixel[1],
  x: pixel[2],
})

/**
 * One `/pixel` lookup per throttle window against the newest pointer position, never one
 * per mousemove, so the readout tracks the cursor within the 100 ms budget. Coordinates
 * update on the move itself; only the values wait on the server.
 */
export class CursorReadout {
  private readonly api: PixelApi
  private readonly throttleMs: number
  private readonly tracks?: TrackLookup
  private readonly now: () => number
  private readonly changed = new Emitter()
  private pending: Target | null = null
  private timer: ReturnType<typeof setTimeout> | null = null
  private inflight = false
  private lastFiredAt = -Infinity
  private current: CursorSample | null = null
  private requests = 0

  constructor(opts: CursorReadoutOptions) {
    this.api = opts.api
    this.throttleMs = opts.throttleMs ?? READOUT_THROTTLE_MS
    this.tracks = opts.tracks
    this.now = opts.now ?? (() => Date.now())
  }

  get sample(): CursorSample | null {
    return this.current
  }

  /** Lookups actually issued — the coalescing assertion in tests. */
  get lookupCount(): number {
    return this.requests
  }

  onChange(cb: () => void): () => void {
    return this.changed.on(cb)
  }

  /** Pointer position in dataset pixels. */
  move(pixel: PixelZYX, ctx: CursorContext): void {
    this.pending = { pixel, t: ctx.t, channel: ctx.channel, labels: ctx.labels ?? false }
    this.current = { ...at(pixel), value: null, labelId: null, trackId: null }
    this.changed.emit()
    this.schedule()
  }

  /** Pointer position on a slice quad, converted through the shared world transform. */
  moveOnSlice(
    world: readonly [number, number],
    ctx: CursorContext & {
      orientation: SliceOrientation
      index: number
      transform: WorldTransform
    },
  ): void {
    const pixel = pixelFromSliceWorld(ctx.orientation, ctx.index, world, ctx.transform)
    this.move([Math.round(pixel[0]), Math.floor(pixel[1]), Math.floor(pixel[2])], ctx)
  }

  clear(): void {
    this.pending = null
    this.current = null
    if (this.timer !== null) clearTimeout(this.timer)
    this.timer = null
    this.changed.emit()
  }

  dispose(): void {
    if (this.timer !== null) clearTimeout(this.timer)
    this.timer = null
    this.pending = null
    this.changed.clear()
  }

  private schedule(): void {
    if (this.inflight || this.timer !== null || !this.pending) return
    const wait = Math.max(0, this.throttleMs - (this.now() - this.lastFiredAt))
    this.timer = setTimeout(() => {
      this.timer = null
      this.fire()
    }, wait)
  }

  private fire(): void {
    const target = this.pending
    if (!target) return
    this.pending = null
    this.inflight = true
    this.lastFiredAt = this.now()
    this.requests += 1
    const [z, y, x] = target.pixel
    const value = this.api
      .pixel({ layer: 'image', t: target.t, c: target.channel, z, y, x })
      .catch(() => null)
    // The label store has one channel and shares the image's coordinate space.
    const label = target.labels
      ? this.api.pixel({ layer: 'labels', t: target.t, c: 0, z, y, x }).catch(() => null)
      : Promise.resolve(null)
    void Promise.all([value, label])
      .then(([intensity, labelValue]) => {
        const labelId = labelValue === null || labelValue === 0 ? null : labelValue
        this.current = {
          ...at(target.pixel),
          value: intensity,
          labelId,
          trackId: labelId === null ? null : (this.tracks?.trackIdFor(labelId) ?? null),
        }
        this.changed.emit()
      })
      .finally(() => {
        this.inflight = false
        this.schedule()
      })
  }
}
