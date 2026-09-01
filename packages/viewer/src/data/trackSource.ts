import type { CellRow } from '@cellstudio/api-client'
import type { PixelApi } from './api'
import { LatestWins } from './prefetch'
import type { TrackFrame } from './trackFrame'

/**
 * Windowed overlay reads: detections for the frames a trail can reach, refetched only
 * when the needed window leaves what is loaded. Whole-graph payloads never cross the API.
 * Requests are keyed by graph version; a response from an older version cannot land after
 * an edit re-versioned the source. */
export class TrackSource {
  private rows: CellRow[] = []
  private byId = new Map<number, CellRow>()
  private loaded: { t0: number; t1: number } | null = null
  private requested: { t0: number; t1: number } | null = null
  /**
   * Window key in flight — `LatestWins` re-invokes work for an unchanged token, so an
   * identical `ensure` stops here.
   */
  private inflight: { key: string } | null = null
  private latest = new LatestWins()
  private listeners = new Set<() => void>()
  private version = 0
  private rev = 0
  /** Rows predate the current graph version: trails may draw them, remaps must not. */
  private stale = false

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

  get graphVersion(): number {
    return this.version
  }

  /** Bumps when the loaded rows change — half of the remap cache key. */
  get revision(): number {
    return this.rev
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

  /** Snapshot for the display remap; never build or cache a remap while `ready` is false. */
  frame(): TrackFrame {
    const span = this.requested ?? this.loaded ?? { t0: 0, t1: 0 }
    return {
      graphVersion: this.version,
      revision: this.rev,
      t0: span.t0,
      t1: span.t1,
      ready: this.ready,
      trackIdFor: (cellId) => this.trackIdFor(cellId),
    }
  }

  private get ready(): boolean {
    return (
      !this.stale &&
      this.loaded !== null &&
      this.requested !== null &&
      this.loaded.t0 <= this.requested.t0 &&
      this.loaded.t1 >= this.requested.t1
    )
  }

  /**
   * Ensures cells for [t − trail, t + trail] are loaded at `graphVersion`. A newer version
   * aborts what is in flight and refetches; requests carry the version in their token so a
   * superseded response cannot land.
   */
  ensure(t: number, trail: number, graphVersion = this.version): void {
    if (graphVersion > this.version) this.setGraphVersion(graphVersion)
    const need = { t0: Math.max(0, t - trail), t1: t + trail }
    this.requested = need
    if (!this.stale && this.loaded && this.loaded.t0 <= need.t0 && this.loaded.t1 >= need.t1) {
      return
    }
    const t0 = Math.max(0, need.t0 - this.margin)
    const t1 = need.t1 + this.margin
    const key = `v${this.version}:${t0}:${t1}`
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
        this.stale = false
        this.rev += 1
        for (const cb of this.listeners) cb()
      })
      .catch(() => {})
      // identity, not the key: a superseded read must not clear its replacement's mark
      .finally(() => {
        if (this.inflight === pending) this.inflight = null
      })
  }

  /**
   * Rows a mask edit changed, applied without a `/cells` round trip — the inspector and
   * the track overlay would otherwise show the pre-stroke centroid and area.   */
  patch(cells: readonly CellRow[], removed: readonly number[]): void {
    if (cells.length === 0 && removed.length === 0) return
    const byId = new Map(this.byId)
    for (const id of removed) byId.delete(id)
    for (const row of cells) byId.set(row.id, row)
    this.byId = byId
    this.rows = [...byId.values()]
    this.rev += 1
    for (const cb of this.listeners) cb()
  }

  /** A committed graph edit: aborts the in-flight read and re-versions future requests. */
  setGraphVersion(version: number): boolean {
    if (version <= this.version) return false
    this.version = version
    this.dropWindow()
    return true
  }

  /**
   * Forces the next `ensure` to refetch. The rows stay on screen for the trails meanwhile,
   * but are stale: `frame().ready` is false until fresh rows land, so remaps never serve
   * them.
   */
  invalidate(): void {
    this.dropWindow()
  }

  /** Drops the rows outright — for a session change, where they belong to another project. */
  reset(): void {
    this.latest.abort()
    this.rows = []
    this.byId = new Map()
    this.loaded = null
    this.requested = null
    this.inflight = null
    this.version = 0
    this.rev += 1
    this.stale = false
    for (const cb of this.listeners) cb()
  }

  dispose(): void {
    this.latest.abort()
    this.inflight = null
    this.listeners.clear()
  }

  private dropWindow(): void {
    this.latest.abort()
    this.inflight = null
    this.loaded = null
    this.stale = true
    for (const cb of this.listeners) cb()
  }
}
