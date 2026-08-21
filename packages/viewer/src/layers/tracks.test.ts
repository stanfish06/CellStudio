import { describe, expect, it } from 'vitest'
import { buildTracks, inSlab, inTrailWindow, trackColor } from './tracks'
import { makeWorldTransform } from '../data/world'
import { cell } from '../test/data'

const scale = { z: 2.0, y: 0.603, x: 0.603 }
const transform = makeWorldTransform(scale, { z: 1, y: 1, x: 1 })

const track = (id: number, zs: number[]) =>
  zs.map((z, i) => cell(id * 100 + i, i, [z, 100 + i * 5, 200 + i * 5], id))

describe('trail window', () => {
  const cells = [cell(1, 0, [1, 10, 10]), cell(2, 6, [1, 10, 10]), cell(3, 12, [1, 10, 10])]

  it('keeps cells within ±K frames of the current one', () => {
    expect(inTrailWindow(cells, 6, 6).map((c) => c.id)).toEqual([1, 2, 3])
    expect(inTrailWindow(cells, 6, 2).map((c) => c.id)).toEqual([2])
    expect(inTrailWindow(cells, 0, 0).map((c) => c.id)).toEqual([1])
  })

  it('shrinks the rendered trail when the window shrinks', () => {
    const cells6 = track(1, [1, 1, 1, 1, 1, 1, 1])
    const wide = buildTracks({ cells: cells6, t: 3, trail: 6, transform })
    const narrow = buildTracks({ cells: cells6, t: 3, trail: 1, transform })
    expect(wide.segments.length).toBeGreaterThan(narrow.segments.length)
    expect(narrow.points.map((p) => p.t).sort()).toEqual([2, 3, 4])
  })
})

describe('slab filtering', () => {
  const cells = [
    cell(1, 0, [0, 10, 10]),
    cell(2, 0, [2, 10, 10]),
    cell(3, 0, [5, 10, 10]),
    { ...cell(4, 0, [0, 0, 0]), centroid: null },
  ]

  it('keeps only cells within the radius of the slice index, on the view normal', () => {
    expect(inSlab(cells, 'xy', 2, 1).map((c) => c.id)).toEqual([2])
    expect(inSlab(cells, 'xy', 2, 3).map((c) => c.id)).toEqual([1, 2, 3])
  })

  it('uses y for XZ and x for YZ', () => {
    const spread = [cell(1, 0, [0, 100, 500]), cell(2, 0, [0, 400, 900])]
    expect(inSlab(spread, 'xz', 100, 5).map((c) => c.id)).toEqual([1])
    expect(inSlab(spread, 'yz', 900, 5).map((c) => c.id)).toEqual([2])
  })

  it('drops centroid-less rows rather than placing them at the origin', () => {
    expect(inSlab(cells, 'xy', 0, 100).map((c) => c.id)).toEqual([1, 2, 3])
  })
})

describe('buildTracks', () => {
  it('projects slice positions onto the two on-screen axes', () => {
    const cells = [cell(1, 0, [2, 300, 700])]
    const xy = buildTracks({
      cells,
      t: 0,
      trail: 0,
      transform,
      orientation: 'xy',
      index: 2,
      slab: 1,
    })
    const xz = buildTracks({
      cells,
      t: 0,
      trail: 0,
      transform,
      orientation: 'xz',
      index: 300,
      slab: 1,
    })
    expect(xy.points[0]?.position).toEqual([700, 300])
    expect(xz.points[0]?.position?.[0]).toBe(700)
    expect(xz.points[0]?.position?.[1]).toBeCloseTo(2 * (2.0 / 0.603), 10)
  })

  it('emits 3D positions in physical units when no orientation is given', () => {
    const cells = [cell(1, 0, [2, 300, 700])]
    const built = buildTracks({ cells, t: 0, trail: 0, transform })
    expect(built.points[0]?.position).toHaveLength(3)
    expect(built.points[0]?.position?.[2]).toBeCloseTo(2 * (2.0 / 0.603), 10)
  })

  it('fades trail segments with distance in t and emphasizes the current frame', () => {
    const cells = track(7, [1, 1, 1, 1, 1])
    const built = buildTracks({ cells, t: 4, trail: 4, transform })
    const alphas = built.segments.map((s) => s.alpha)
    expect(alphas[alphas.length - 1]).toBeGreaterThan(alphas[0] as number)
    expect(built.points.filter((p) => p.current)).toHaveLength(1)
  })

  it('colors by track identity, deterministically', () => {
    const a = buildTracks({ cells: track(3, [1, 1]), t: 1, trail: 2, transform })
    const b = buildTracks({ cells: track(3, [1, 1]), t: 1, trail: 2, transform })
    expect(a.points[0]?.color).toEqual(b.points[0]?.color)
    expect(trackColor(3)).not.toEqual(trackColor(4))
    expect(trackColor(11).every((c) => c >= 0 && c <= 255)).toBe(true)
  })

  it('highlights the selected lineage distinctly', () => {
    const cells = track(9, [1, 1, 1])
    const built = buildTracks({
      cells,
      t: 1,
      trail: 2,
      transform,
      lineage: new Set([cells[1]?.id ?? -1]),
    })
    const selected = built.points.find((p) => p.selected)
    expect(selected?.color).toEqual([255, 255, 255])
    expect(built.segments.some((s) => s.selected)).toBe(true)
  })

  it('falls back to the cell id when a row has no track', () => {
    const orphan = { ...cell(42, 0, [1, 1, 1]), trackId: null }
    const built = buildTracks({ cells: [orphan], t: 0, trail: 0, transform })
    expect(built.points[0]?.trackId).toBe(42)
  })
})
