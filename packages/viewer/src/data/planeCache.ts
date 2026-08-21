import type { LayerId, PlaneBuffer } from '@cellstudio/api-client'
import type { PerfSink } from '../perf'
import type { PixelApi } from './api'
import { ByteLru } from './lru'
import { planeKeyId, staleOf, type PlaneKey } from './keys'
import { DEFAULT_BRICK, orthoPrefetchIndices, type BrickShape } from './prefetch'
import { RequestPool } from './requests'

export interface PlaneCacheOptions {
  api: PixelApi
  /** Bytes of assembled planes retained. */
  capacity?: number
  brick?: BrickShape
  maxConcurrent?: number
  perf?: PerfSink
}

/**
 * The XZ/YZ path: whole server-assembled planes keyed on their full identity, so a
 * version bump or a channel-set change is a different plane rather than a stale hit.
 */
export class PlaneCache {
  private readonly api: PixelApi
  private readonly lru: ByteLru<PlaneBuffer>
  private readonly pool: RequestPool<PlaneBuffer>
  private readonly brick: BrickShape
  private readonly perf?: PerfSink
  private pendingKeys = new Map<string, PlaneKey>()

  constructor(opts: PlaneCacheOptions) {
    this.api = opts.api
    this.brick = opts.brick ?? DEFAULT_BRICK
    this.perf = opts.perf
    this.lru = new ByteLru<PlaneBuffer>(opts.capacity ?? 256 * 1024 * 1024)
    this.pool = new RequestPool<PlaneBuffer>(
      (id, signal) => this.fetch(id, signal),
      opts.maxConcurrent ?? 6,
    )
  }

  get stats() {
    return { entries: this.lru.size, bytes: this.lru.bytes, ...this.pool.stats }
  }

  peek(key: PlaneKey): PlaneBuffer | undefined {
    return this.lru.peek(planeKeyId(key))
  }

  has(key: PlaneKey): boolean {
    return this.lru.has(planeKeyId(key))
  }

  async get(key: PlaneKey, signal?: AbortSignal): Promise<PlaneBuffer> {
    const id = planeKeyId(key)
    const hit = this.lru.get(id)
    if (hit) return hit
    this.pendingKeys.set(id, key)
    return this.pool.request(id, 'active', signal)
  }

  /** Warm one exact plane — used for inactive-view t-settle warming. */
  warm(key: PlaneKey): void {
    const id = planeKeyId(key)
    if (this.lru.has(id)) return
    this.pendingKeys.set(id, key)
    this.pool.request(id, 'prefetch').catch(() => {})
  }

  /**
   * Warm along the scrub direction: the adjacent plane plus one plane in the next brick.
   * `maxIndex` bounds the axis; omit it only when the caller has already clamped.
   */
  prefetch(key: PlaneKey, dir: 1 | -1, maxIndex = Number.MAX_SAFE_INTEGER): void {
    const extent = key.axis === 'xz' ? this.brick.y : this.brick.x
    for (const index of orthoPrefetchIndices(key.index, dir, extent, maxIndex)) {
      this.warm({ ...key, index })
    }
  }

  /** Drop every plane of `layer` that is not at `version`. */
  invalidate(layer: LayerId, version: number): number {
    return this.lru.deleteWhere(staleOf(layer, version))
  }

  clear(): void {
    this.pool.abortAll()
    this.lru.clear()
    this.pendingKeys.clear()
  }

  private async fetch(id: string, signal: AbortSignal): Promise<PlaneBuffer> {
    const key = this.pendingKeys.get(id)
    if (!key) throw new Error(`plane request without key: ${id}`)
    const span = this.perf?.span(id, 'network')
    const plane = await this.api.slice(
      {
        layer: key.layer,
        axis: key.axis,
        t: key.t,
        cs: [...key.c],
        pos: key.index,
        level: key.level,
      },
      signal,
    )
    span?.end(plane.data.byteLength)
    this.pendingKeys.delete(id)
    this.lru.set(id, plane, plane.data.byteLength)
    return plane
  }
}
