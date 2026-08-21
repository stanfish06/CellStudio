import { describe, expect, it } from 'vitest'
import { PlaneCache } from './planeCache'
import { planeKeyId, type PlaneKey } from './keys'
import { FakeApi, makePlane } from '../test/data'
import { isAbortError } from './requests'

const key = (over: Partial<PlaneKey> = {}): PlaneKey => ({
  layer: 'image',
  axis: 'xz',
  level: 0,
  t: 4,
  c: [0, 1],
  index: 512,
  version: 1,
  ...over,
})

describe('PlaneCache', () => {
  it('serves a second request for the same identity from cache', async () => {
    const api = new FakeApi()
    const cache = new PlaneCache({ api })
    await cache.get(key())
    await cache.get(key())
    expect(api.sliceCalls).toHaveLength(1)
    expect(cache.has(key())).toBe(true)
  })

  it('treats a different channel set as a different plane', async () => {
    const api = new FakeApi()
    const cache = new PlaneCache({ api })
    await cache.get(key({ c: [0, 1] }))
    await cache.get(key({ c: [0] }))
    expect(api.sliceCalls.map((c) => c.q.cs)).toEqual([[0, 1], [0]])
  })

  it('misses after a version bump and drops the stale entry', async () => {
    const api = new FakeApi()
    const cache = new PlaneCache({ api })
    await cache.get(key({ version: 1 }))
    expect(cache.invalidate('image', 2)).toBe(1)
    expect(cache.has(key({ version: 1 }))).toBe(false)
    await cache.get(key({ version: 2 }))
    expect(api.sliceCalls).toHaveLength(2)
  })

  it('leaves another layer alone on invalidation', async () => {
    const api = new FakeApi()
    const cache = new PlaneCache({ api })
    await cache.get(key({ layer: 'labels' }))
    expect(cache.invalidate('image', 9)).toBe(0)
    expect(cache.has(key({ layer: 'labels' }))).toBe(true)
  })

  it('issues one request for identical concurrent gets', async () => {
    const api = new FakeApi()
    api.auto = false
    const cache = new PlaneCache({ api })
    const a = cache.get(key())
    const b = cache.get(key())
    expect(api.sliceCalls).toHaveLength(1)
    api.settleSlice(0, makePlane(3, 1024, 2))
    expect((await a).shape).toEqual([3, 1024])
    expect((await b).shape).toEqual([3, 1024])
  })

  it('aborts a superseded request', async () => {
    const api = new FakeApi()
    api.auto = false
    const cache = new PlaneCache({ api })
    const controller = new AbortController()
    const superseded = cache.get(key({ index: 512 }), controller.signal)
    controller.abort()
    await expect(superseded).rejects.toSatisfy(isAbortError)
    expect(api.sliceSignal(0)?.aborted).toBe(true)
  })

  it('warms the adjacent plane and the next brick along the scrub direction', () => {
    const api = new FakeApi()
    const cache = new PlaneCache({ api, brick: { z: 16, y: 256, x: 256 } })
    cache.prefetch(key({ index: 500 }), 1, 1023)
    expect(api.sliceCalls.map((c) => c.q.pos)).toEqual([501, 512])
  })

  it('does not prefetch past the axis bound', () => {
    const api = new FakeApi()
    const cache = new PlaneCache({ api })
    cache.prefetch(key({ index: 1023 }), 1, 1023)
    expect(api.sliceCalls).toHaveLength(0)
  })

  it('skips a warming read for a plane it already holds', async () => {
    const api = new FakeApi()
    const cache = new PlaneCache({ api })
    await cache.get(key({ index: 501 }))
    cache.prefetch(key({ index: 500 }), 1, 1023)
    expect(api.sliceCalls.map((c) => c.q.pos)).toEqual([501, 512])
  })

  it('reports cache bytes and evicts under its capacity', async () => {
    const api = new FakeApi()
    const planeBytes = 3 * 1024 * 2 * 2
    const cache = new PlaneCache({ api, capacity: planeBytes })
    await cache.get(key({ index: 1 }))
    await cache.get(key({ index: 2 }))
    expect(cache.stats.entries).toBe(1)
    expect(cache.has(key({ index: 1 }))).toBe(false)
    expect(cache.stats.bytes).toBe(planeBytes)
  })

  it('keys the request on the plane identity it was asked for', async () => {
    const api = new FakeApi()
    const cache = new PlaneCache({ api })
    await cache.get(key({ axis: 'yz', level: 2, t: 9, index: 7 }))
    expect(api.sliceCalls[0]?.q).toEqual({
      layer: 'image',
      axis: 'yz',
      t: 9,
      cs: [0, 1],
      pos: 7,
      level: 2,
    })
    expect(planeKeyId(key({ axis: 'yz', level: 2, t: 9, index: 7 }))).toBe('image/yz/2/9/0.1/7/v1')
  })
})
