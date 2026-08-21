import type { CellRow } from '@cellstudio/api-client'
import type { PixelApi } from './api'
import { LatestWins } from './prefetch'

/**
 * Windowed overlay reads: detections for the frames a trail can reach, refetched only
 * when the needed window leaves what is loaded. Whole-graph payloads never cross the API.
 */
export class TrackSource {
  private rows: CellRow[] = []
  private byId = new Map<number, CellRow>()
  private loaded: { t0: number; t1: number } | null = null
  /**
   * Window key in flight — `LatestWins` re-invokes work for an unchanged token and
   * `cellsWindow` drops the signal, so an identical `ensure` stops here.
   */
  private inflight: { key: string } | null = null
  private latest = new LatestWins()
  private listeners = new Set<() => void>()

  constructor(
    private readonly api: PixelApi,
    /** Extra frames fetched either side, so small t-steps stay local. */
    private readonly margin = 8,
  ) {}

  get cells(): readonly CellRow[] {
    return this.rows
  }

  get window(): { t0: number; t1: number } | null {
    return this.loaded
  }

  /** A cell id is a label voxel value; null when it is outside the loaded window. */
  cell(cellId: number): CellRow | null {
    return this.byId.get(cellId) ?? null
  }

  trackIdFor(cellId: number): number | null {
    return this.byId.get(cellId)?.trackId ?? null
  }

  onChange(cb: () => void): () => void {
    this.listeners.add(cb)
    return () => this.listeners.delete(cb)
  }

  /** Ensures cells for [t − trail, t + trail] are loaded. */
  ensure(t: number, trail: number): void {
    const need = { t0: t - trail, t1: t + trail }
    if (this.loaded && this.loaded.t0 <= need.t0 && this.loaded.t1 >= need.t1) return
    const t0 = Math.max(0, need.t0 - this.margin)
    const t1 = need.t1 + this.margin
    const key = `${t0}:${t1}`
    if (this.inflight?.key === key) return
    const pending = { key }
    this.inflight = pending
    void this.latest
      .run(key, (signal) => this.api.cellsWindow({ t0, t1 }, signal))
      .then((cells) => {
        if (!cells) return
        this.rows = cells
        this.byId = new Map(cells.map((c) => [c.id, c]))
        this.loaded = { t0, t1 }
        for (const cb of this.listeners) cb()
      })
      .catch(() => {})
      // identity, not the key: a superseded read must not clear its replacement's mark
      .finally(() => {
        if (this.inflight === pending) this.inflight = null
      })
  }

  /** Forces the next `ensure` to refetch, keeping the current rows on screen meanwhile. */
  invalidate(): void {
    this.loaded = null
    this.inflight = null
  }

  /** Drops the rows outright — for a session change, where they belong to another project. */
  reset(): void {
    this.latest.abort()
    this.rows = []
    this.byId = new Map()
    this.loaded = null
    this.inflight = null
    for (const cb of this.listeners) cb()
  }

  dispose(): void {
    this.latest.abort()
    this.inflight = null
    this.listeners.clear()
  }
}
