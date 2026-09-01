import type { CellRow, LineageTree } from '@cellstudio/api-client'
import { describe, expect, it } from 'vitest'
import { isLowConfidence, lineageLayout, lineageRows, lineageSpan } from './lineage'

const cell = (id: number, t: number, confidence: number | null = 0.9): CellRow => ({
  id,
  t,
  centroid: [17, 1048, 983],
  area: 1462,
  confidence,
  state: null,
  trackId: 94,
  parentId: null,
  reviewed: false,
})

const tree: LineageTree = {
  rootCellId: 1788,
  focusCellId: 1788,
  cells: [cell(1889, 131), cell(1788, 118), cell(1933, 136), cell(1932, 136), cell(1842, 126, 0.5)],
  links: [
    { parent: 1788, child: 1842, confidence: 0.82, reviewed: false },
    { parent: 1842, child: 1889, confidence: 0.61, reviewed: false },
    { parent: 1889, child: 1932, confidence: 0.88, reviewed: false },
    { parent: 1889, child: 1933, confidence: 0.79, reviewed: false },
  ],
}

describe('lineageRows', () => {
  it('orders by frame then id', () => {
    expect(lineageRows(tree, null).map((r) => r.cell.id)).toEqual([1788, 1842, 1889, 1932, 1933])
  })

  it('reports the incoming link confidence, falling back to detection confidence at the root', () => {
    const byId = new Map(lineageRows(tree, null).map((r) => [r.cell.id, r]))
    expect(byId.get(1788)?.confidence).toBe(0.9)
    expect(byId.get(1842)?.confidence).toBe(0.82)
    expect(byId.get(1932)?.confidence).toBe(0.88)
  })

  it('marks the cell with two children as a division', () => {
    expect(
      lineageRows(tree, null)
        .filter((r) => r.division)
        .map((r) => r.cell.id),
    ).toEqual([1889])
  })

  it('marks the selected cell', () => {
    const rows = lineageRows(tree, 1842)
    expect(rows.filter((r) => r.selected).map((r) => r.cell.id)).toEqual([1842])
  })

  it('has no rows without a lineage', () => {
    expect(lineageRows(null, 1842)).toEqual([])
    expect(lineageSpan([])).toBe('—')
  })
})

describe('lineageSpan', () => {
  it('spans the first and last frame of the lineage', () => {
    expect(lineageSpan(lineageRows(tree, null))).toBe('T 118–136')
  })
})

describe('isLowConfidence', () => {
  it('flags links a proofreader should look at', () => {
    expect(isLowConfidence(0.61)).toBe(true)
    expect(isLowConfidence(0.88)).toBe(false)
    expect(isLowConfidence(null)).toBe(false)
  })
})

describe('lineageLayout', () => {
  const layout = lineageLayout(tree, 1842)
  const node = (id: number) => layout?.nodes.find((n) => n.cell.id === id)

  it('assigns one column per branch, post-order by leaf count', () => {
    expect(layout?.columns).toBe(2)
    expect(node(1932)?.column).toBe(0)
    expect(node(1933)?.column).toBe(1)
  })

  it('keeps a chain in its column and centres the division over its children', () => {
    expect(node(1889)?.column).toBe(0.5)
    expect(node(1842)?.column).toBe(0.5)
    expect(node(1788)?.column).toBe(0.5)
    expect(node(1889)?.division).toBe(true)
  })

  it('places nodes vertically in proportion to t', () => {
    expect(layout?.tMin).toBe(118)
    expect(layout?.tMax).toBe(136)
    expect(node(1788)?.row).toBe(0)
    expect(node(1842)?.row).toBe(126 - 118)
    expect(node(1889)?.row).toBe(131 - 118)
    expect(node(1932)?.row).toBe(136 - 118)
    expect(node(1933)?.row).toBe(136 - 118)
    // time-ordered node list, so the tree reads top to bottom
    const rows = layout?.nodes.map((n) => n.row) ?? []
    expect(rows).toEqual([...rows].sort((a, b) => a - b))
  })

  it('emits one edge per link, including both division edges', () => {
    expect(layout?.edges).toHaveLength(4)
    expect(layout?.edges.filter((e) => e.parent.id === 1889).map((e) => e.child.id)).toEqual([
      1932, 1933,
    ])
  })

  it('marks the selected node and the incoming-link confidence', () => {
    expect(node(1842)?.selected).toBe(true)
    expect(node(1889)?.selected).toBe(false)
    expect(node(1842)?.confidence).toBe(0.82)
    expect(node(1788)?.confidence).toBe(0.9)
  })

  it('lays out a single detached cell as one node in one column', () => {
    const single = lineageLayout(
      { rootCellId: 7, focusCellId: 7, cells: [cell(7, 12)], links: [] },
      null,
    )
    expect(single?.columns).toBe(1)
    expect(single?.nodes).toHaveLength(1)
    expect(single?.nodes[0]).toMatchObject({ column: 0, row: 0 })
  })

  it('is null without a lineage', () => {
    expect(lineageLayout(null, 1)).toBe(null)
  })
})
