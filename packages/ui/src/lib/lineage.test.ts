import type { CellRow, LineageTree } from '@cellstudio/api-client'
import { describe, expect, it } from 'vitest'
import { isLowConfidence, lineageRows, lineageSpan } from './lineage'

const cell = (id: number, t: number, confidence: number | null = 0.9): CellRow => ({
  id,
  t,
  centroid: [17, 1048, 983],
  area: 1462,
  confidence,
  state: null,
  trackId: 94,
  reviewed: false,
})

const tree: LineageTree = {
  rootCellId: 1788,
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
