import type { Dims } from '@cellstudio/api-client'
import { describe, expect, it } from 'vitest'
import {
  clampIndex,
  nextPlaybackIndex,
  parseIndex,
  progressPercent,
  transportRows,
} from './transport'

const dims: Dims = { t: 400, c: 2, z: 45, y: 2048, x: 2048 }

describe('transportRows', () => {
  // The caller maps the active view to its axis with the nav store's `sliceAxis`.
  it('gives an XY view (slice axis z) a time row and a Z row', () => {
    const rows = transportRows({ t: 126, slice: { axis: 'z', index: 17 }, dims })
    expect(rows.map((r) => r.label)).toEqual(['T', 'Z'])
    expect(rows[0]).toMatchObject({ axis: 't', value: 126, max: 399, editable: true })
    expect(rows[1]).toMatchObject({ axis: 'z', value: 17, max: 44, editable: false })
  })

  it('bounds the slice row by its own axis — y for XZ, x for YZ', () => {
    expect(transportRows({ t: 0, slice: { axis: 'y', index: 1048 }, dims })[1]).toMatchObject({
      axis: 'y',
      label: 'Y',
      value: 1048,
      max: 2047,
    })
    expect(transportRows({ t: 0, slice: { axis: 'x', index: 983 }, dims })[1]).toMatchObject({
      axis: 'x',
      label: 'X',
      value: 983,
      max: 2047,
    })
  })

  it('hides the slice row in 3D — no slice axis — and keeps the time row', () => {
    const rows = transportRows({ t: 126, slice: null, dims })
    expect(rows).toHaveLength(1)
    expect(rows[0]).toMatchObject({ axis: 't', value: 126, max: 399, editable: true })
  })

  it('degrades to zero-length axes before a project is open', () => {
    const rows = transportRows({ t: 0, slice: { axis: 'z', index: 0 }, dims: null })
    expect(rows.map((r) => r.max)).toEqual([0, 0])
  })
})

describe('clampIndex', () => {
  it('rounds and clamps into [0, max]', () => {
    expect(clampIndex(126.4, 399)).toBe(126)
    expect(clampIndex(126.6, 399)).toBe(127)
    expect(clampIndex(-5, 399)).toBe(0)
    expect(clampIndex(1e9, 399)).toBe(399)
    expect(clampIndex(5, 0)).toBe(0)
    expect(clampIndex(Number.NaN, 399)).toBe(0)
  })
})

describe('parseIndex', () => {
  it('accepts a typed frame number and clamps it to the axis', () => {
    expect(parseIndex('42', 399)).toBe(42)
    expect(parseIndex(' 42 ', 399)).toBe(42)
    expect(parseIndex('42.7', 399)).toBe(43)
    expect(parseIndex('4000', 399)).toBe(399)
    expect(parseIndex('-12', 399)).toBe(0)
  })

  it('rejects text that is not a frame number', () => {
    expect(parseIndex('', 399)).toBeNull()
    expect(parseIndex('   ', 399)).toBeNull()
    expect(parseIndex('abc', 399)).toBeNull()
    expect(parseIndex('12px', 399)).toBeNull()
  })
})

describe('progressPercent', () => {
  it('reports the slider fill and never leaves [0,100]', () => {
    expect(progressPercent(126, 399)).toBe('31.6%')
    expect(progressPercent(0, 399)).toBe('0%')
    expect(progressPercent(399, 399)).toBe('100%')
    expect(progressPercent(500, 399)).toBe('100%')
    expect(progressPercent(1, 0)).toBe('0%')
  })
})

describe('nextPlaybackIndex', () => {
  it('advances one step and wraps at the end of the axis', () => {
    expect(nextPlaybackIndex(0, 44)).toBe(1)
    expect(nextPlaybackIndex(43, 44)).toBe(44)
    expect(nextPlaybackIndex(44, 44)).toBe(0)
    expect(nextPlaybackIndex(0, 0)).toBe(0)
  })
})
