import type { CellRow } from '@cellstudio/api-client'
import { describe, expect, it } from 'vitest'
import { TrackSource } from './trackSource'
import { FakeApi, cell } from '../test/data'

const flush = () => new Promise((r) => setTimeout(r, 0))

/** `FakeApi` answers `cellsWindow` at once; these tests need the window held in flight. */
class HeldApi extends FakeApi {
  signals: (AbortSignal | undefined)[] = []
  private pending: ((rows: CellRow[]) => void)[] = []

  cellsWindow(q: { t0: number; t1: number }, signal?: AbortSignal): Promise<CellRow[]> {
    this.cellCalls.push({ t0: q.t0, t1: q.t1 })
    this.signals.push(signal)
    return new Promise<CellRow[]>((resolve) => this.pending.push(resolve))
  }

  settle(at = 0): void {
    const resolve = this.pending[at]
    if (!resolve) throw new Error(`no pending window at ${at}`)
    resolve(this.cells)
  }
}

describe('TrackSource', () => {
  it('reads once for repeated ensures of the window already in flight', async () => {
    const api = new HeldApi()
    const tracks = new TrackSource(api)
    // a camera drag or a t-scrub before the first window lands
    for (let i = 0; i < 8; i += 1) tracks.ensure(20, 6)
    expect(api.cellCalls).toEqual([{ t0: 6, t1: 34 }])
    api.settle()
    await flush()
    expect(api.cellCalls).toHaveLength(1)
  })

  it('supersedes the in-flight window when the trail widens', () => {
    const api = new HeldApi()
    const tracks = new TrackSource(api)
    tracks.ensure(20, 6)
    tracks.ensure(20, 40)
    expect(api.cellCalls).toEqual([
      { t0: 6, t1: 34 },
      { t0: 0, t1: 68 },
    ])
  })

  it('makes no read once the loaded window covers what is asked for', async () => {
    const api = new HeldApi()
    api.cells = [cell(77, 20, [1, 10, 20], 5)]
    const tracks = new TrackSource(api)
    tracks.ensure(20, 6)
    api.settle()
    await flush()
    expect(tracks.window).toEqual({ t0: 6, t1: 34 })
    tracks.ensure(20, 6)
    tracks.ensure(21, 5)
    expect(api.cellCalls).toHaveLength(1)
    expect(tracks.trackIdFor(77)).toBe(5)
  })

  it('refetches the same window once after invalidate, keeping the rows meanwhile', async () => {
    const api = new HeldApi()
    api.cells = [cell(77, 20, [1, 10, 20], 5)]
    const tracks = new TrackSource(api)
    tracks.ensure(20, 6)
    api.settle()
    await flush()
    tracks.invalidate()
    tracks.ensure(20, 6)
    tracks.ensure(20, 6)
    expect(api.cellCalls).toHaveLength(2)
    expect(tracks.cells).toHaveLength(1)
  })

  it('clears the in-flight key on reset', () => {
    const api = new HeldApi()
    const tracks = new TrackSource(api)
    tracks.ensure(20, 6)
    tracks.reset()
    tracks.ensure(20, 6)
    expect(api.cellCalls).toHaveLength(2)
  })
})

describe('TrackSource graph versioning', () => {
  it('is not ready until the requested interval has arrived — labels can beat /cells', async () => {
    const api = new HeldApi()
    api.cells = [cell(77, 20, [1, 10, 20], 5)]
    const tracks = new TrackSource(api)
    expect(tracks.frame().ready).toBe(false)
    tracks.ensure(20, 6, 1)
    expect(tracks.frame().ready).toBe(false)
    api.settle()
    await flush()
    const frame = tracks.frame()
    expect(frame).toMatchObject({ ready: true, graphVersion: 1, t0: 14, t1: 26 })
    expect(frame.trackIdFor(77)).toBe(5)
  })

  it('aborts the in-flight read on a version advance, and the stale response never lands', async () => {
    const api = new HeldApi()
    api.cells = [cell(77, 20, [1, 10, 20], 5)]
    const tracks = new TrackSource(api)
    tracks.ensure(20, 6, 7)
    expect(tracks.setGraphVersion(8)).toBe(true)
    expect(api.signals[0]?.aborted).toBe(true)
    // the v7 response resolves anyway; it must not land under v8
    api.settle(0)
    await flush()
    expect(tracks.frame().ready).toBe(false)
    expect(tracks.cells).toHaveLength(0)

    tracks.ensure(20, 6, 8)
    expect(api.cellCalls).toHaveLength(2)
    api.settle(1)
    await flush()
    expect(tracks.frame()).toMatchObject({ ready: true, graphVersion: 8 })
    expect(tracks.trackIdFor(77)).toBe(5)
  })

  it('marks loaded rows stale on invalidate while keeping them for the trails', async () => {
    const api = new HeldApi()
    api.cells = [cell(77, 20, [1, 10, 20], 5)]
    const tracks = new TrackSource(api)
    tracks.ensure(20, 6)
    api.settle()
    await flush()
    expect(tracks.frame().ready).toBe(true)
    tracks.invalidate()
    expect(tracks.frame().ready).toBe(false)
    expect(tracks.cells).toHaveLength(1)
  })

  it('rejects a non-advancing version', () => {
    const tracks = new TrackSource(new HeldApi())
    expect(tracks.setGraphVersion(3)).toBe(true)
    expect(tracks.setGraphVersion(3)).toBe(false)
    expect(tracks.setGraphVersion(2)).toBe(false)
    expect(tracks.graphVersion).toBe(3)
  })

  it('bumps the revision when rows land or a patch applies', async () => {
    const api = new HeldApi()
    api.cells = [cell(77, 20, [1, 10, 20], 5)]
    const tracks = new TrackSource(api)
    const before = tracks.revision
    tracks.ensure(20, 6)
    api.settle()
    await flush()
    expect(tracks.revision).toBe(before + 1)
    tracks.patch([cell(78, 20, [1, 11, 21], 5)], [])
    expect(tracks.revision).toBe(before + 2)
  })
})

describe('TrackSource fetch policy (task 5.2)', () => {
  const loaded = async () => {
    const api = new HeldApi()
    api.cells = [cell(77, 20, [1, 10, 20], 5)]
    const tracks = new TrackSource(api)
    tracks.ensure(20, 10, 1)
    api.settle()
    await flush()
    expect(api.cellCalls).toHaveLength(1)
    return { api, tracks }
  }

  it('a trail change within the loaded window makes zero /cells calls', async () => {
    const { api, tracks } = await loaded()
    tracks.ensure(20, 4, 1)
    tracks.ensure(20, 12, 1)
    // the 8-frame margin covers trail 12 around t=20: [8, 32] ⊆ [2, 38]
    expect(api.cellCalls).toHaveLength(1)
  })

  it('an uncovered expansion issues exactly one versioned request', async () => {
    const { api, tracks } = await loaded()
    tracks.ensure(20, 30, 1)
    tracks.ensure(20, 30, 1)
    expect(api.cellCalls).toHaveLength(2)
    expect(api.cellCalls[1]).toEqual({ t0: 0, t1: 58 })
    api.settle(1)
    await flush()
    expect(tracks.frame()).toMatchObject({ ready: true, graphVersion: 1 })
  })
})
