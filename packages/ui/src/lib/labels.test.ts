import type { CellRow, LabelDefinition } from '@cellstudio/api-client'
import { describe, expect, it } from 'vitest'
import {
  removeHint,
  statesFromRow,
  toggleBody,
  validateLabelName,
  withColor,
  withDefinition,
} from './labels'

const defs: LabelDefinition[] = [
  { name: 'cell type 1', uses: 6 },
  { name: 'verified', uses: 1 },
  { name: 'unused', uses: 0 },
]

const row: CellRow = {
  id: 581,
  t: 1,
  centroid: [1, 7, 163],
  area: 90,
  confidence: 0.9,
  state: null,
  trackId: 1,
  parentId: null,
  reviewed: false,
  labels: ['verified'],
  trackLabels: ['cell type 1'],
}

describe('validateLabelName', () => {
  it('trims and refuses empty or duplicate names', () => {
    expect(validateLabelName('  new  ', defs)).toEqual({ name: 'new' })
    expect(validateLabelName('   ', defs)).toEqual({ error: 'A label needs a name' })
    expect(validateLabelName('verified ', defs)).toEqual({
      error: '"verified" is already defined',
    })
  })
})

describe('withDefinition', () => {
  it('appends with a default colour, keeps existing colours, and sorts', () => {
    const coloured = [{ ...defs[0]!, color: '#112233' }, ...defs.slice(1)]
    const next = withDefinition(coloured, 'alpha')
    expect(next.map((d) => d.name)).toEqual(['alpha', 'cell type 1', 'unused', 'verified'])
    expect(next[0]?.color).toMatch(/^#[0-9a-f]{6}$/)
    expect(next[1]?.color).toBe('#112233')
    expect(next[2]?.color).toBeNull()
  })
})

describe('withColor', () => {
  it("replaces one entry's colour and nothing else", () => {
    const next = withColor(defs, 'verified', '#abcdef')
    expect(next.find((d) => d.name === 'verified')?.color).toBe('#abcdef')
    expect(next.find((d) => d.name === 'unused')?.color).toBeNull()
    expect(next).toHaveLength(3)
  })
})

describe('toggleBody', () => {
  it('removes a fully applied label and adds anything else', () => {
    const on = { name: 'verified', cell: true, track: 'all' as const }
    const partial = { name: 'verified', cell: false, track: 'some' as const }
    expect(toggleBody(on, 'cell', 581)).toEqual({
      cellId: 581,
      scope: 'cell',
      remove: ['verified'],
    })
    expect(toggleBody(on, 'track', 581)).toEqual({
      cellId: 581,
      scope: 'track',
      remove: ['verified'],
    })
    expect(toggleBody(partial, 'cell', 581)).toEqual({
      cellId: 581,
      scope: 'cell',
      add: ['verified'],
    })
    expect(toggleBody(partial, 'track', 581)).toEqual({
      cellId: 581,
      scope: 'track',
      add: ['verified'],
    })
  })
})

describe('statesFromRow', () => {
  it('derives first-paint states from the row and none without a row', () => {
    expect(statesFromRow(defs, row)).toEqual([
      { name: 'cell type 1', cell: false, track: 'all' },
      { name: 'verified', cell: true, track: 'none' },
      { name: 'unused', cell: false, track: 'none' },
    ])
    expect(statesFromRow(defs, null).every((s) => !s.cell && s.track === 'none')).toBe(true)
  })
})

describe('removeHint', () => {
  it('names the strip and its size', () => {
    expect(removeHint(defs[2]!)).toBe('Remove "unused"')
    expect(removeHint(defs[1]!)).toBe('Remove "verified" from 1 cell (undoable)')
    expect(removeHint(defs[0]!)).toBe('Remove "cell type 1" from 6 cells (undoable)')
  })
})
