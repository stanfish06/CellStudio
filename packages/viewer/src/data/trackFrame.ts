import type { Dtype, PlaneBuffer, VolumeBuffer } from '@cellstudio/api-client'
import { ByteLru } from './lru'

/**
 * A versioned, readiness-aware view of the graph identity window A remap is
 * never built or cached while `ready` is false — a label buffer that beats `/cells` renders
 * canonically and swaps to the remap when the window lands.
 */
export interface TrackFrame {
  graphVersion: number
  /** Bumps when the window's rows change (fetch landed, mask-edit patch). */
  revision: number
  t0: number
  t1: number
  /** The requested interval at the requested graph version has arrived. */
  ready: boolean
  trackIdFor(cellId: number): number | null
}

/**
 * The same frame with highlighted cells redirected to `HIGHLIGHT_BASE + slot`, so the
 * remap paints them in a highlight colour instead of their track's. Cache keys must carry
 * the slot signature alongside the version, since the frame's own fields do not change.
 */
export function withHighlightSlots(
  frame: TrackFrame,
  slots: ReadonlyMap<number, number>,
  base: number,
  stride = 1,
): TrackFrame {
  if (slots.size === 0) return frame
  return {
    ...frame,
    trackIdFor: (cellId: number) => {
      const slot = slots.get(cellId)
      return slot === undefined ? frame.trackIdFor(cellId) : base + slot * stride
    },
  }
}

/**
 * Display copies of the label planes/volumes on screen, u32. Half of `PlaneCache`'s
 * 256 MB: the remap holds only what is being drawn, not the scrub history.
 */
export const REMAP_CAPACITY_BYTES = 128 * 1024 * 1024

const viewOf = (data: ArrayBuffer, dtype: Dtype): Uint8Array | Uint16Array | Uint32Array =>
  dtype === 'u8'
    ? new Uint8Array(data)
    : dtype === 'u16'
      ? new Uint16Array(data)
      : new Uint32Array(data)

/**
 * Voxel id → `trackIdFor(id) ?? id`; 0 stays background. Unknown and freshly painted ids
 * keep id-coloring, which is what lets the mask-editor echo render before `/cells` knows
 * the new cell. `out` reuses a buffer of the exact output size instead of allocating.
 */
export function remapToTracks(
  data: ArrayBuffer,
  dtype: Dtype,
  frame: TrackFrame,
  out?: ArrayBuffer,
): ArrayBuffer {
  const src = viewOf(data, dtype)
  const buffer = out && out.byteLength === src.length * 4 ? out : new ArrayBuffer(src.length * 4)
  const dst = new Uint32Array(buffer)
  // Label data runs in spans of one id; the last-value shortcut makes the memo per-span.
  const memo = new Map<number, number>()
  let lastIn = 0
  let lastOut = 0
  for (let i = 0; i < src.length; i += 1) {
    const v = src[i] as number
    if (v !== lastIn) {
      lastIn = v
      const hit = memo.get(v)
      if (hit === undefined) {
        lastOut = v === 0 ? 0 : (frame.trackIdFor(v) ?? v)
        memo.set(v, lastOut)
      } else {
        lastOut = hit
      }
    }
    dst[i] = lastOut
  }
  return buffer
}

/**
 * Byte-accounted cache of remapped display buffers, keyed `(buffer key, graphVersion,
 * revision)`. Canonical buffers pass through untouched while the frame is not ready.
 * `cacheable: false` is for editor-synthesized buffers, which change under an unchanged
 * buffer key while a stroke is live — those remap into a per-key scratch buffer instead.
 */
export class RemapCache {
  private readonly lru: ByteLru<PlaneBuffer | VolumeBuffer>
  private readonly scratch = new Map<string, ArrayBuffer>()

  constructor(capacity = REMAP_CAPACITY_BYTES) {
    this.lru = new ByteLru(capacity)
  }

  get stats() {
    return { entries: this.lru.size, bytes: this.lru.bytes }
  }

  plane(bufferKey: string, plane: PlaneBuffer, frame: TrackFrame, cacheable = true): PlaneBuffer {
    if (!frame.ready) return plane
    const key = this.key(bufferKey, frame)
    if (cacheable) {
      const hit = this.lru.get(key)
      if (hit) return hit as PlaneBuffer
    }
    const remapped: PlaneBuffer = {
      shape: plane.shape,
      channels: plane.channels,
      dtype: 'u32',
      level: plane.level,
      data: this.remap(bufferKey, plane.data, plane.dtype, frame, cacheable),
    }
    if (cacheable) this.lru.set(key, remapped, remapped.data.byteLength)
    return remapped
  }

  volume(
    bufferKey: string,
    volume: VolumeBuffer,
    frame: TrackFrame,
    cacheable = true,
  ): VolumeBuffer {
    if (!frame.ready) return volume
    const key = this.key(bufferKey, frame)
    if (cacheable) {
      const hit = this.lru.get(key)
      if (hit) return hit as VolumeBuffer
    }
    const remapped: VolumeBuffer = {
      shape: volume.shape,
      dtype: 'u32',
      level: volume.level,
      data: this.remap(bufferKey, volume.data, volume.dtype, frame, cacheable),
    }
    if (cacheable) this.lru.set(key, remapped, remapped.data.byteLength)
    return remapped
  }

  clear(): void {
    this.lru.clear()
    this.scratch.clear()
  }

  private key(bufferKey: string, frame: TrackFrame): string {
    return `${bufferKey}|g${frame.graphVersion}|r${frame.revision}`
  }

  private remap(
    bufferKey: string,
    data: ArrayBuffer,
    dtype: Dtype,
    frame: TrackFrame,
    cacheable: boolean,
  ): ArrayBuffer {
    if (cacheable) {
      // Reuse the superseded revision's buffer for this key — every element is rewritten.
      const stale = this.lru
        .keys()
        .filter((k) => k.startsWith(`${bufferKey}|`))
        .map((k) => this.lru.peek(k))
      let out: ArrayBuffer | undefined
      const bytes = viewOf(data, dtype).length * 4
      for (const entry of stale) {
        if (entry && entry.data.byteLength === bytes) out = entry.data
      }
      this.lru.deleteWhere((k) => k.startsWith(`${bufferKey}|`))
      return remapToTracks(data, dtype, frame, out)
    }
    const bytes = viewOf(data, dtype).length * 4
    const scratchKey = `${bufferKey}:${bytes}`
    let out = this.scratch.get(scratchKey)
    if (!out) {
      out = new ArrayBuffer(bytes)
      this.scratch.set(scratchKey, out)
    }
    return remapToTracks(data, dtype, frame, out)
  }
}
