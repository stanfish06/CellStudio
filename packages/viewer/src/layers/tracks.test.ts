import { describe, expect, it } from 'vitest'
import {
  SELECTED_COLOR,
  buildLineageEdges,
  buildTracks,
  highlightSlots,
  labeledCells,
  inSlab,
  inTrailWindow,
  shownTracks,
  trackColor,
  trackSpans,
  type LineageOverlay,
  withLineageEdges,
} from './tracks'
import { LABEL_PALETTE, LABEL_PALETTE_SIZE, labelColor, trackPaletteIndex } from './labelPalette'
import { makeWorldTransform } from '../data/world'
import { cell } from '../test/data'

const scale = { z: 2.0, y: 0.603, x: 0.603 }
const transform = makeWorldTransform(scale, { z: 1, y: 1, x: 1 })

const track = (id: number, zs: number[]) =>
  zs.map((z, i) => cell(id * 100 + i, i, [z, 100 + i * 5, 200 + i * 5], id))

describe('trail window', () => {
  const cells = [cell(1, 0, [1, 10, 10]), cell(2, 6, [1, 10, 10]), cell(3, 12, [1, 10, 10])]

  it('keeps cells within the backward window [t − K, t], never future ones', () => {
    expect(inTrailWindow(cells, 6, 6).map((c) => c.id)).toEqual([1, 2])
    expect(inTrailWindow(cells, 12, 6).map((c) => c.id)).toEqual([2, 3])
    expect(inTrailWindow(cells, 6, 2).map((c) => c.id)).toEqual([2])
    expect(inTrailWindow(cells, 0, 0).map((c) => c.id)).toEqual([1])
  })

  it('shrinks the rendered trail when the window shrinks', () => {
    const cells6 = track(1, [1, 1, 1, 1, 1, 1, 1])
    const wide = buildTracks({ cells: cells6, t: 3, trail: 6, transform })
    const narrow = buildTracks({ cells: cells6, t: 3, trail: 1, transform })
    expect(wide.segments.length).toBeGreaterThan(narrow.segments.length)
    expect(narrow.points.map((p) => p.t).sort()).toEqual([2, 3])
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

  it('fades trail segments with age by default and emphasizes the current frame', () => {
    const cells = track(7, [1, 1, 1, 1, 1])
    const built = buildTracks({ cells, t: 4, trail: 4, transform })
    const alphas = built.segments.map((s) => s.alpha)
    expect(alphas[alphas.length - 1]).toBe(1)
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

  it('colors trails from the shared label palette, keyed by the track id', () => {
    // mask and trail agree by construction: both read LABEL_PALETTE[trackPaletteIndex]
    const built = buildTracks({ cells: track(3, [1, 1]), t: 1, trail: 2, transform })
    expect(built.points[0]?.color).toEqual(labelColor(3))
    expect(built.segments[0]?.color).toEqual(labelColor(3))
    const entry = LABEL_PALETTE[trackPaletteIndex(3)] as readonly number[]
    expect(built.points[0]?.color).toEqual(entry.map((c) => Math.round(c * 255)))
  })

  it('keys the palette exactly as the label shader does: (id − 1) mod palette size', () => {
    expect(trackPaletteIndex(1)).toBe(0)
    expect(trackPaletteIndex(LABEL_PALETTE_SIZE)).toBe(LABEL_PALETTE_SIZE - 1)
    expect(trackPaletteIndex(LABEL_PALETTE_SIZE + 1)).toBe(0)
    expect(trackColor(7 + LABEL_PALETTE_SIZE)).toEqual(trackColor(7))
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

  it('colours cells carrying a highlighted label, with the selection winning', () => {
    const cells = track(9, [1, 1, 1]).map((c, i) => ({
      ...c,
      labels: i === 1 ? ['verified'] : [],
      trackLabels: i === 2 ? ['verified'] : [],
    }))
    const amber: [number, number, number] = [255, 191, 105]
    const highlighted = labeledCells(cells, 2, [{ name: 'verified', color: amber }])
    expect([...(highlighted?.keys() ?? [])]).toEqual([cells[2]?.id])
    expect(labeledCells(cells, 1, [{ name: 'verified', color: amber }])?.size).toBe(1)
    expect(labeledCells(cells, 1, [])).toBeUndefined()
    const built = buildTracks({
      cells,
      t: 2,
      trail: 2,
      transform,
      highlighted,
      lineage: new Set([cells[2]?.id ?? -1]),
    })
    const point = built.points.find((p) => p.cellId === cells[2]?.id)
    expect(point?.highlighted).toBe(true)
    expect(point?.color).toEqual([255, 255, 255])
    const alone = buildTracks({ cells, t: 2, trail: 2, transform, highlighted })
    expect(alone.points.find((p) => p.cellId === cells[2]?.id)?.color).toEqual(amber)
  })

  it('assigns highlight slots per label in sheet order, capped at the shader slot count', () => {
    const cells = track(9, [1, 1, 1]).map((c, i) => ({
      ...c,
      labels: i === 0 ? ['a'] : i === 1 ? ['a', 'b'] : ['c'],
      trackLabels: [],
    }))
    const hl = (name: string, color: [number, number, number]) => ({ name, color })
    const { slots, colors, signature } = highlightSlots(
      cells,
      1,
      [hl('b', [1, 1, 1]), hl('a', [2, 2, 2]), hl('c', [3, 3, 3])],
      2,
    )
    expect(slots.get(cells[1]?.id ?? -1)).toBe(0)
    expect(slots.has(cells[0]?.id ?? -1)).toBe(false)
    expect(colors).toEqual([
      [1, 1, 1],
      [2, 2, 2],
    ])
    expect(signature).toContain('1.1.1|2.2.2#')
    expect(highlightSlots(cells, 1, [], 8).slots.size).toBe(0)
  })

  it('falls back to the cell id when a row has no track', () => {
    const orphan = { ...cell(42, 0, [1, 1, 1]), trackId: null }
    const built = buildTracks({ cells: [orphan], t: 0, trail: 0, transform })
    expect(built.points[0]?.trackId).toBe(42)
  })
})

describe('trail decay', () => {
  // One track over t = 0..5; a segment's frame is its newer endpoint (toCellId % 100).
  const cells = track(5, [1, 1, 1, 1, 1, 1])
  const segT = (s: { toCellId: number }) => s.toCellId % 100
  const fade = { on: true, max: 0.9, min: 0.2 }

  it('draws no segment or point from a future frame', () => {
    const built = buildTracks({ cells, t: 3, trail: 10, transform })
    expect(built.segments.every((s) => segT(s) <= 3)).toBe(true)
    expect(built.points.every((p) => p.t <= 3)).toBe(true)
  })

  it('renders exactly max at the segment ending at the current frame', () => {
    const built = buildTracks({ cells, t: 5, trail: 4, transform, fade })
    expect(built.segments.find((s) => segT(s) === 5)?.alpha).toBe(0.9)
  })

  it('renders exactly min at the segment whose newer endpoint is t − trail', () => {
    const built = buildTracks({ cells, t: 5, trail: 4, transform, fade })
    expect(built.segments.find((s) => segT(s) === 1)?.alpha).toBe(0.2)
  })

  it('interpolates linearly between the bounds', () => {
    const built = buildTracks({ cells, t: 5, trail: 4, transform, fade })
    expect(built.segments.find((s) => segT(s) === 3)?.alpha).toBeCloseTo(0.55, 10)
  })

  it('renders a single segment at the current frame at max', () => {
    const pair = track(6, [1, 1])
    const wide = buildTracks({ cells: pair, t: 1, trail: 10, transform, fade })
    expect(wide.segments).toHaveLength(1)
    expect(wide.segments[0]?.alpha).toBe(0.9)
    const zero = buildTracks({ cells: pair, t: 1, trail: 0, transform, fade })
    expect(zero.segments[0]?.alpha).toBe(0.9)
  })

  it('renders every segment at max when decay is off', () => {
    const built = buildTracks({
      cells,
      t: 5,
      trail: 5,
      transform,
      fade: { on: false, max: 0.7, min: 0.1 },
    })
    expect(built.segments).toHaveLength(5)
    expect(built.segments.every((s) => s.alpha === 0.7)).toBe(true)
  })
})

describe('ended tracks', () => {
  // track 492 lives t = 0..10 and ends; track 7 lives t = 0..30
  const ended = Array.from({ length: 11 }, (_, t) => cell(100 + t, t, [1, 10, 10 + t], 492))
  const alive = Array.from({ length: 31 }, (_, t) => cell(200 + t, t, [1, 50, 50 + t], 7))
  const cells = [...ended, ...alive]

  it('spans a track over every loaded row, future frames included', () => {
    expect(trackSpans(cells).get(492)).toEqual({ first: 0, last: 10, parent: null })
    expect(trackSpans(cells).get(7)).toEqual({ first: 0, last: 30, parent: null })
  })

  it('draws nothing for a track that ended before the current frame', () => {
    const built = buildTracks({ cells, t: 16, trail: 10, transform })
    expect(built.segments.some((s) => s.trackId === 492)).toBe(false)
    expect(built.points.some((p) => p.trackId === 492)).toBe(false)
    // newer endpoints t = 6..16, the one at t − trail included
    expect(built.segments.filter((s) => s.trackId === 7)).toHaveLength(11)
  })

  it('still draws the track on its last frame', () => {
    const built = buildTracks({ cells, t: 10, trail: 10, transform })
    expect(built.segments.filter((s) => s.trackId === 492)).toHaveLength(10)
  })

  it('keeps a track visible through a detection gap', () => {
    const gapped = alive.filter((c) => c.t !== 16)
    const built = buildTracks({ cells: gapped, t: 16, trail: 10, transform })
    // newer endpoints t = 6..15; the pair across the gap ends in the future and is not drawn
    expect(built.segments.filter((s) => s.trackId === 7)).toHaveLength(10)
    expect(built.points.some((p) => p.current)).toBe(false)
  })

  it('hides a selected track that ended, the same as any other', () => {
    const built = buildTracks({
      cells,
      t: 16,
      trail: 10,
      transform,
      lineage: new Set(ended.map((c) => c.id)),
    })
    expect(built.segments.some((s) => s.selected)).toBe(false)
  })
})

describe('daughter trails extend into the parent track', () => {
  // parent track 5 over t = 0..4 divides; daughters 6 and 7 start at t = 5 and run to t = 12
  const parent = Array.from({ length: 5 }, (_, t) => cell(10 + t, t, [1, 100, 100 + t], 5))
  const last = parent[4] as ReturnType<typeof cell>
  const daughter = (track: number, y: number) =>
    Array.from({ length: 8 }, (_, i) =>
      cell(track * 100 + i, 5 + i, [1, y, 110 + i], track, i === 0 ? last.id : track * 100 + i - 1),
    )
  const a = daughter(6, 90)
  const b = daughter(7, 110)
  const cells = [...parent, ...a, ...b]

  it('names the parent track in the daughter spans', () => {
    const spans = trackSpans(cells)
    expect(spans.get(6)?.parent).toBe(5)
    expect(spans.get(5)?.parent).toBeNull()
    expect([...shownTracks(spans, 8)].sort()).toEqual([5, 6, 7])
  })

  it('draws the parent trail while a daughter is alive, with the division edges', () => {
    const built = buildTracks({ cells, t: 8, trail: 6, transform })
    // parent segments with newer endpoint at t ≥ 2: (1→2), (2→3), (3→4)
    expect(built.segments.filter((s) => s.trackId === 5)).toHaveLength(3)
    const edges = built.segments.filter((s) => s.fromCellId === last.id)
    expect(edges.map((e) => e.toCellId).sort()).toEqual([600, 700])
    expect(edges.map((e) => e.trackId).sort()).toEqual([6, 7])
    expect(edges[0]?.color).toEqual(trackColor(edges[0]?.trackId ?? -1))
    expect(built.points.filter((p) => p.trackId === 5).map((p) => p.t)).toEqual([2, 3, 4])
  })

  it('drops the parent once every daughter has ended', () => {
    const built = buildTracks({ cells, t: 14, trail: 20, transform })
    expect(built.segments).toEqual([])
    expect(built.points).toEqual([])
  })

  it('keeps the parent when only one daughter survives', () => {
    const cut = [...parent, ...a, ...b.slice(0, 3)]
    const built = buildTracks({ cells: cut, t: 10, trail: 20, transform })
    expect(built.segments.some((s) => s.trackId === 5)).toBe(true)
    expect(built.segments.some((s) => s.trackId === 7)).toBe(false)
    expect(built.segments.some((s) => s.trackId === 6)).toBe(true)
  })

  it('whitens the division edge into a selected daughter', () => {
    const built = buildTracks({ cells, t: 8, trail: 6, transform, lineage: new Set([600, 14]) })
    const edge = built.segments.find((s) => s.fromCellId === 14 && s.toCellId === 600)
    expect(edge?.selected).toBe(true)
    expect(edge?.color).toEqual(SELECTED_COLOR)
  })

  it('merges only the overlay edges the rows could not draw, under the same gate', () => {
    const lineage: LineageOverlay = {
      graphVersion: 1,
      focusCellId: 600,
      cells: [last, a[0] as ReturnType<typeof cell>],
      links: [{ parent: last.id, child: 600 }],
    }
    const built = buildTracks({ cells, t: 8, trail: 6, transform })
    const merged = withLineageEdges(built, cells, { lineage, t: 8, trail: 6, transform })
    expect(merged.segments.filter((s) => s.toCellId === 600)).toHaveLength(1)
    // the parent cell is missing from the rows: the overlay supplies the edge
    const without = cells.filter((c) => c.id !== last.id)
    const partial = withLineageEdges(
      buildTracks({ cells: without, t: 8, trail: 6, transform }),
      without,
      { lineage, t: 8, trail: 6, transform },
    )
    expect(partial.segments.filter((s) => s.toCellId === 600)).toHaveLength(1)
    // once the daughter has ended, the gate drops the overlay edge too
    const ended = withLineageEdges(
      buildTracks({ cells: without, t: 14, trail: 20, transform }),
      without,
      { lineage, t: 14, trail: 20, transform },
    )
    expect(ended.segments).toEqual([])
  })
})

describe('buildLineageEdges.', () => {
  // A division at t = 2: parent track 5, children head tracks 6 and 7.
  const parent = cell(20, 2, [1, 100, 100], 5)
  const childA = cell(30, 3, [1, 110, 110], 6)
  const childB = cell(31, 3, [4, 90, 90], 7)
  const lineage: LineageOverlay = {
    graphVersion: 1,
    focusCellId: 20,
    cells: [cell(10, 1, [1, 95, 95], 5), parent, childA, childB],
    links: [
      { parent: 10, child: 20 }, // same track: already a trail segment
      { parent: 20, child: 30 },
      { parent: 20, child: 31 },
    ],
  }

  it('emits one segment per cross-track link when both endpoints are in-window', () => {
    const edges = buildLineageEdges({ lineage, t: 3, trail: 4, transform })
    expect(edges.map((e) => [e.fromCellId, e.toCellId])).toEqual([
      [20, 30],
      [20, 31],
    ])
    expect(edges.every((e) => e.selected)).toBe(true)
    expect(edges.every((e) => e.color === SELECTED_COLOR)).toBe(true)
  })

  it('never draws a same-track link — buildTracks already renders those', () => {
    const edges = buildLineageEdges({ lineage, t: 3, trail: 10, transform })
    expect(edges.some((e) => e.toCellId === 20)).toBe(false)
  })

  it('applies the backward trail window on the child endpoint', () => {
    // child at t = 3: future at t = 2, in-window through t = 7, aged out at t = 8
    expect(buildLineageEdges({ lineage, t: 2, trail: 4, transform })).toEqual([])
    expect(buildLineageEdges({ lineage, t: 7, trail: 4, transform })).toHaveLength(2)
    expect(buildLineageEdges({ lineage, t: 8, trail: 4, transform })).toEqual([])
  })

  it('fades the edge exactly like a trail segment of the same age', () => {
    const fade = { on: true, max: 1, min: 0.2 }
    const atNow = buildLineageEdges({ lineage, t: 3, trail: 4, transform, fade })
    expect(atNow[0]?.alpha).toBe(1)
    const atEdge = buildLineageEdges({ lineage, t: 7, trail: 4, transform, fade })
    expect(atEdge[0]?.alpha).toBe(0.2)
  })

  it('respects the slab filter of the slice views', () => {
    // around z = 4 only childB is in the slab; the edge to childA (both endpoints z = 1) drops
    const at = (index: number) =>
      buildLineageEdges({ lineage, t: 3, trail: 4, transform, orientation: 'xy', index, slab: 1 })
    expect(at(4).map((e) => e.toCellId)).toEqual([31])
    // either endpoint inside the slab keeps the edge, exactly like trail segments
    expect(at(1).map((e) => e.toCellId)).toEqual([30, 31])
    expect(at(10)).toEqual([])
  })

  it('builds nothing without a lineage', () => {
    expect(buildLineageEdges({ lineage: null, t: 3, trail: 4, transform })).toEqual([])
    expect(buildLineageEdges({ lineage: undefined, t: 3, trail: 4, transform })).toEqual([])
  })
})
