import type { CellRow } from '@cellstudio/api-client'
import { describe, expect, it } from 'vitest'
import { TrackSource } from './trackSource'
import { FakeApi, cell } from '../test/data'

const flush = () => new Promise((r) => setTimeout(r, 0))

/** `FakeApi` answers `cellsWindow` at once; these tests need the window held in flight. */
class HeldApi extends FakeApi {
  private pending: ((rows: CellRow[]) => void)[] = []

  cellsWindow(q: { t0: number; t1: number }): Promise<CellRow[]> {
    this.cellCalls.push({ t0: q.t0, t1: q.t1 })
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
