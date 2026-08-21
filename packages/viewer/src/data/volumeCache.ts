import type { LayerId, VolumeBuffer } from '@cellstudio/api-client'
import type { PerfSink } from '../perf'
import type { PixelApi } from './api'
import { ByteLru } from './lru'
import { staleOf, volumeKeyId, type VolumeKey } from './keys'
import { RequestPool } from './requests'

export interface VolumeContext {
  layer: LayerId
  level: number
  version: number
  /** Visible channels, one volume each. */
  channels: number[]
  /** Highest valid t. */
  tMax: number
}

export interface VolumeCacheOptions {
  api: PixelApi
  /** Bytes of raw volume buffers retained — GPU textures are viv's, not ours. */
  capacity?: number
  maxConcurrent?: number
  perf?: PerfSink
}

/** ArrayBuffer LRU for `/volume` responses; the volume layer owns the 3D textures. */
export class VolumeCache {
  private readonly api: PixelApi
  private readonly lru: ByteLru<VolumeBuffer>
  private readonly pool: RequestPool<VolumeBuffer>
  private readonly perf?: PerfSink
  private pendingKeys = new Map<string, VolumeKey>()
  private context: VolumeContext | null = null

  constructor(opts: VolumeCacheOptions) {
    this.api = opts.api
    this.perf = opts.perf
    this.lru = new ByteLru<VolumeBuffer>(opts.capacity ?? 512 * 1024 * 1024)
    this.pool = new RequestPool<VolumeBuffer>(
      (id, signal) => this.fetch(id, signal),
      opts.maxConcurrent ?? 4,
    )
  }

  get stats() {
    return { entries: this.lru.size, bytes: this.lru.bytes, ...this.pool.stats }
  }

  /** What `prefetch(t)` should fetch: current layer, level, version and visible channels. */
  configure(context: VolumeContext): void {
    this.context = context
  }

  resize(capacity: number): void {
    this.lru.resize(capacity)
  }

  peek(key: VolumeKey): VolumeBuffer | undefined {
    return this.lru.peek(volumeKeyId(key))
  }

  has(key: VolumeKey): boolean {
    return this.lru.has(volumeKeyId(key))
  }

  async get(key: VolumeKey, signal?: AbortSignal): Promise<VolumeBuffer> {
    const id = volumeKeyId(key)
    const hit = this.lru.get(id)
    if (hit) return hit
    this.pendingKeys.set(id, key)
    return this.pool.request(id, 'active', signal)
  }

  warm(key: VolumeKey): void {
    const id = volumeKeyId(key)
    if (this.lru.has(id)) return
    this.pendingKeys.set(id, key)
    this.pool.request(id, 'prefetch').catch(() => {})
  }

  /** Warm every visible channel's volume at `t`. */
  prefetch(t: number): void {
    const ctx = this.context
    if (!ctx || t < 0 || t > ctx.tMax) return
    for (const c of ctx.channels) {
      this.warm({ layer: ctx.layer, level: ctx.level, version: ctx.version, t, c })
    }
  }

  invalidate(layer: LayerId, version: number): number {
    return this.lru.deleteWhere(staleOf(layer, version))
  }

  clear(): void {
    this.pool.abortAll()
    this.lru.clear()
    this.pendingKeys.clear()
  }

  private async fetch(id: string, signal: AbortSignal): Promise<VolumeBuffer> {
    const key = this.pendingKeys.get(id)
    if (!key) throw new Error(`volume request without key: ${id}`)
    const span = this.perf?.span(id, 'network')
    const volume = await this.api.volume(
      { layer: key.layer, t: key.t, c: key.c, level: key.level },
      signal,
    )
    span?.end(volume.data.byteLength)
    this.pendingKeys.delete(id)
    this.lru.set(id, volume, volume.data.byteLength)
    return volume
  }
}
