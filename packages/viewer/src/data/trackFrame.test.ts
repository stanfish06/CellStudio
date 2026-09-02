import { describe, expect, it } from 'vitest'
import { RemapCache, type TrackFrame, remapToTracks, withHighlightSlots } from './trackFrame'
import { makeLabelPlane, makeLabelVolume } from '../test/data'
import type { PlaneBuffer } from '@cellstudio/api-client'

const frameOf = (
  map: Record<number, number>,
  o: Partial<Omit<TrackFrame, 'trackIdFor'>> = {},
): TrackFrame => ({
  graphVersion: 1,
  revision: 1,
  t0: 0,
  t1: 10,
  ready: true,
  trackIdFor: (id) => map[id] ?? null,
  ...o,
})

const plane = (ids: number[]): PlaneBuffer => {
  const out = makeLabelPlane(1, ids.length)
  new Uint32Array(out.data).set(ids)
  return out
}

const values = (buffer: { data: ArrayBuffer }): number[] => [...new Uint32Array(buffer.data)]

describe('remapToTracks', () => {
  it('maps voxel ids to track ids, keeps background, and leaves unknown ids alone', () => {
    const data = remapToTracks(
      new Uint32Array([0, 77, 77, 78, 0, 999]).buffer,
      'u32',
      frameOf({ 77: 5, 78: 5 }),
    )
    expect([...new Uint32Array(data)]).toEqual([0, 5, 5, 5, 0, 999])
  })

  it('widens narrower dtypes to u32', () => {
    const data = remapToTracks(new Uint16Array([0, 3, 3]).buffer, 'u16', frameOf({ 3: 70000 }))
    expect([...new Uint32Array(data)]).toEqual([0, 70000, 70000])
  })

  it('is the identity when no cells are loaded — the no-tracks fallback', () => {
    const ids = [0, 1, 2, 1024, 42]
    const data = remapToTracks(new Uint32Array(ids).buffer, 'u32', frameOf({}))
    expect([...new Uint32Array(data)]).toEqual(ids)
  })

  it('writes into a supplied buffer of the exact size', () => {
    const out = new ArrayBuffer(3 * 4)
    const data = remapToTracks(new Uint32Array([1, 2, 3]).buffer, 'u32', frameOf({ 1: 9 }), out)
    expect(data).toBe(out)
    expect([...new Uint32Array(out)]).toEqual([9, 2, 3])
  })
})

describe('RemapCache', () => {
  it('passes the canonical buffer through and caches nothing before ready', () => {
    // A label plane that beats /cells must not freeze fallback colors under this key.
    const cache = new RemapCache()
    const p = plane([0, 77])
    const notReady = frameOf({ 77: 5 }, { ready: false })
    expect(cache.plane('k', p, notReady)).toBe(p)
    expect(cache.stats.entries).toBe(0)
    // when the frame becomes ready the same key serves the remap
    const shown = cache.plane('k', p, frameOf({ 77: 5 }))
    expect(values(shown)).toEqual([0, 5])
    expect(cache.stats.entries).toBe(1)
  })

  it('serves the cached copy for an unchanged (key, graphVersion, revision)', () => {
    const cache = new RemapCache()
    const p = plane([77])
    const frame = frameOf({ 77: 5 })
    const first = cache.plane('k', p, frame)
    expect(cache.plane('k', p, frame)).toBe(first)
    expect(cache.stats.entries).toBe(1)
  })

  it('recomputes when the revision moves and drops the superseded entry', () => {
    const cache = new RemapCache()
    const p = plane([77])
    expect(values(cache.plane('k', p, frameOf({ 77: 5 }, { revision: 1 })))).toEqual([5])
    expect(values(cache.plane('k', p, frameOf({ 77: 6 }, { revision: 2 })))).toEqual([6])
    expect(cache.stats.entries).toBe(1)
  })

  it('keys on the graph version, so a v7 remap is never served under v8', () => {
    const cache = new RemapCache()
    const p = plane([77])
    expect(values(cache.plane('k', p, frameOf({ 77: 5 }, { graphVersion: 7 })))).toEqual([5])
    expect(values(cache.plane('k', p, frameOf({ 77: 9 }, { graphVersion: 8 })))).toEqual([9])
    // and asking again at v8 serves v8's mapping, not a revived v7 entry
    expect(values(cache.plane('k', p, frameOf({ 77: 9 }, { graphVersion: 8 })))).toEqual([9])
  })

  it('never caches an editor-synthesized buffer, and reuses its scratch allocation', () => {
    const cache = new RemapCache()
    const frame = frameOf({ 77: 5 })
    const a = cache.plane('k', plane([77, 1]), frame, false)
    const b = cache.plane('k', plane([1, 77]), frame, false)
    expect(cache.stats.entries).toBe(0)
    expect(values(b)).toEqual([1, 5])
    expect(b.data).toBe(a.data)
  })

  it('remaps volumes on the same rules', () => {
    const cache = new RemapCache()
    const v = makeLabelVolume(1, 1, 4, 0)
    new Uint32Array(v.data).set([0, 77, 78, 3])
    const shown = cache.volume('vk', v, frameOf({ 77: 5, 78: 5 }))
    expect(values(shown)).toEqual([0, 5, 5, 3])
    expect(shown.shape).toEqual(v.shape)
    expect(cache.stats.bytes).toBe(16)
  })

  it('evicts by bytes and empties on clear', () => {
    const cache = new RemapCache(8)
    const frame = frameOf({})
    cache.plane('a', plane([1]), frame)
    cache.plane('b', plane([2]), frame)
    cache.plane('c', plane([3]), frame)
    expect(cache.stats.bytes).toBeLessThanOrEqual(8)
    cache.clear()
    expect(cache.stats).toEqual({ entries: 0, bytes: 0 })
  })
})

describe('withHighlightSlots', () => {
  it('redirects slotted cells to the highlight range and leaves the rest to the frame', () => {
    const frame = {
      graphVersion: 1,
      revision: 1,
      t0: 0,
      t1: 0,
      ready: true,
      trackIdFor: (id: number) => (id === 7 ? 70 : null),
    }
    const shown = withHighlightSlots(frame, new Map([[9, 1]]), 1000)
    expect(shown.trackIdFor(9)).toBe(1001)
    expect(shown.trackIdFor(7)).toBe(70)
    expect(shown.trackIdFor(8)).toBe(null)
    expect(withHighlightSlots(frame, new Map(), 1000)).toBe(frame)
  })
})
