import { isAbortError } from './requests'
import type { PlaneKey, VolumeKey } from './keys'

export interface BrickShape {
  z: number
  y: number
  x: number
}

export const DEFAULT_BRICK: BrickShape = { z: 16, y: 256, x: 256 }

export const brickStart = (index: number, extent: number): number =>
  Math.floor(index / Math.max(1, extent)) * Math.max(1, extent)

/**
 * Near edge of the next brick along `dir`, or null past the end. One read here warms a
 * whole brick of the scrub axis, which is the prefetch unit for the raw XY path.
 */
export function nextBrickIndex(
  index: number,
  dir: 1 | -1,
  extent: number,
  max: number,
): number | null {
  const step = Math.max(1, extent)
  const start = brickStart(index, step)
  const target = dir === 1 ? start + step : start - 1
  if (target < 0 || target > max) return null
  return target
}

/**
 * Ortho scrubbing warms the adjacent plane (so the next step is a client hit) and one
 * plane in the next brick (so crossing the brick boundary is not a cold server decode).
 */
export function orthoPrefetchIndices(
  index: number,
  dir: 1 | -1,
  extent: number,
  max: number,
): number[] {
  const adjacent = index + dir
  const candidates = [adjacent, nextBrickIndex(index, dir, extent, max)]
  const out: number[] = []
  for (const c of candidates) {
    if (c === null || c < 0 || c > max || c === index || out.includes(c)) continue
    out.push(c)
  }
  return out
}

export interface WarmPlan {
  planes: PlaneKey[]
  volumes: VolumeKey[]
}

export interface WarmSinks {
  plane(key: PlaneKey): void
  volume(key: VolumeKey): void
}

/**
 * Inactive views warm on t-settle rather than per step, so a fast scrub does not drag
 * volume buffers behind it. Each schedule replaces the pending plan.
 */
export class TSettleWarmer {
  private timer: ReturnType<typeof setTimeout> | null = null
  private plan: WarmPlan | null = null

  constructor(
    private readonly sinks: WarmSinks,
    private readonly delayMs = 150,
  ) {}

  get pending(): boolean {
    return this.timer !== null
  }

  schedule(plan: WarmPlan): void {
    this.plan = plan
    if (this.timer !== null) clearTimeout(this.timer)
    this.timer = setTimeout(() => {
      this.timer = null
      this.flush()
    }, this.delayMs)
  }

  flush(): void {
    const plan = this.plan
    this.plan = null
    if (!plan) return
    for (const key of plan.planes) this.sinks.plane(key)
    for (const key of plan.volumes) this.sinks.volume(key)
  }

  cancel(): void {
    if (this.timer !== null) clearTimeout(this.timer)
    this.timer = null
    this.plan = null
  }
}

/**
 * Latest-key-wins gate: a result commits only while its token is still the newest, and
 * the superseded request is aborted.
 */
export class LatestWins {
  private controller: AbortController | null = null
  private current: string | null = null

  get token(): string | null {
    return this.current
  }

  async run<T>(token: string, work: (signal: AbortSignal) => Promise<T>): Promise<T | null> {
    if (this.current !== token) {
      this.controller?.abort()
      this.controller = new AbortController()
      this.current = token
    }
    const signal = (this.controller as AbortController).signal
    try {
      const value = await work(signal)
      return this.current === token ? value : null
    } catch (e) {
      if (isAbortError(e)) return null
      throw e
    }
  }

  abort(): void {
    this.controller?.abort()
    this.controller = null
    this.current = null
  }
}
