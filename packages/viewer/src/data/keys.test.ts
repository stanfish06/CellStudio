import { describe, expect, it } from 'vitest'
import { planeKeyId, samePlaneKey, staleOf, volumeKeyId, type PlaneKey } from './keys'

const base: PlaneKey = {
  layer: 'image',
  axis: 'xz',
  level: 0,
  t: 12,
  c: [0, 2],
  index: 511,
  version: 3,
}

describe('plane key identity', () => {
  it('distinguishes every field of the full identity', () => {
    const variants: PlaneKey[] = [
      { ...base, layer: 'labels' },
      { ...base, axis: 'yz' },
      { ...base, level: 1 },
      { ...base, t: 13 },
      { ...base, c: [0, 1] },
      { ...base, c: [0] },
      { ...base, index: 512 },
      { ...base, version: 4 },
    ]
    const ids = new Set([planeKeyId(base), ...variants.map(planeKeyId)])
    expect(ids.size).toBe(variants.length + 1)
    for (const v of variants) expect(samePlaneKey(base, v)).toBe(false)
  })

  it('is stable for an equal key built separately', () => {
    expect(planeKeyId({ ...base, c: [0, 2] })).toBe(planeKeyId(base))
  })

  it('keys volumes on layer, level, t, channel and version', () => {
    expect(volumeKeyId({ layer: 'image', level: 2, t: 5, c: 1, version: 7 })).toBe('image/2/5/1/v7')
  })
})

describe('version invalidation', () => {
  it('matches only keys of that layer at another version', () => {
    const stale = staleOf('image', 4)
    expect(stale(planeKeyId(base))).toBe(true)
    expect(stale(planeKeyId({ ...base, version: 4 }))).toBe(false)
    expect(stale(planeKeyId({ ...base, layer: 'labels' }))).toBe(false)
  })
})
