import { describe, expect, it } from 'vitest'
import { VolumeCache } from './volumeCache'
import type { VolumeKey } from './keys'
import { FakeApi, makeVolume } from '../test/data'
import { isAbortError } from './requests'

const key = (over: Partial<VolumeKey> = {}): VolumeKey => ({
  layer: 'image',
  level: 2,
  t: 10,
  c: 0,
  version: 1,
  ...over,
})

const context = { layer: 'image' as const, level: 2, version: 1, channels: [0, 2], tMax: 276 }

describe('VolumeCache', () => {
  it('caches on the full key and re-requests after a version bump', async () => {
    const api = new FakeApi()
    const cache = new VolumeCache({ api })
    await cache.get(key())
    await cache.get(key())
    expect(api.volumeCalls).toHaveLength(1)
    cache.invalidate('image', 2)
    await cache.get(key({ version: 2 }))
    expect(api.volumeCalls).toHaveLength(2)
  })

  it('prefetches one volume per visible channel at the requested t', () => {
    const api = new FakeApi()
    const cache = new VolumeCache({ api })
    cache.configure(context)
    cache.prefetch(11)
    expect(api.volumeCalls.map((c) => ({ t: c.q.t, c: c.q.c }))).toEqual([
      { t: 11, c: 0 },
      { t: 11, c: 2 },
    ])
  })

  it('ignores a prefetch outside the time axis', () => {
    const api = new FakeApi()
    const cache = new VolumeCache({ api })
    cache.configure(context)
    cache.prefetch(-1)
    cache.prefetch(277)
    expect(api.volumeCalls).toHaveLength(0)
  })

  it('does nothing until it has been configured', () => {
    const api = new FakeApi()
    new VolumeCache({ api }).prefetch(5)
    expect(api.volumeCalls).toHaveLength(0)
  })

  it('shares an in-flight volume between a prefetch and the active view', async () => {
    const api = new FakeApi()
    api.auto = false
    const cache = new VolumeCache({ api })
    cache.configure(context)
    cache.prefetch(11)
    const active = cache.get(key({ t: 11, c: 0 }))
    expect(api.volumeCalls).toHaveLength(2)
    api.settleVolume(0, makeVolume(3, 256, 256))
    expect((await active).shape).toEqual([3, 256, 256])
  })

  it('aborts a superseded volume request', async () => {
    const api = new FakeApi()
    api.auto = false
    const cache = new VolumeCache({ api })
    const controller = new AbortController()
    const superseded = cache.get(key(), controller.signal)
    controller.abort()
    await expect(superseded).rejects.toSatisfy(isAbortError)
    expect(api.volumeCalls[0]?.signal?.aborted).toBe(true)
  })

  it('bounds itself by bytes', async () => {
    const api = new FakeApi()
    const bytes = 3 * 256 * 256 * 2
    const cache = new VolumeCache({ api, capacity: bytes })
    await cache.get(key({ t: 1 }))
    await cache.get(key({ t: 2 }))
    expect(cache.stats.entries).toBe(1)
    expect(cache.has(key({ t: 2 }))).toBe(true)
  })
})
