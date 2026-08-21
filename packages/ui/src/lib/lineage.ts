import type { CellRow, LineageTree } from '@cellstudio/api-client'

export interface LineageRow {
  cell: CellRow
  /** Confidence of the link into this cell, or the detection confidence when it is a root. */
  confidence: number | null
  division: boolean
  selected: boolean
}

/** Time-ordered rows with division markers for the Lineage tab. */
export function lineageRows(tree: LineageTree | null, selectedId: number | null): LineageRow[] {
  if (!tree) return []
  const childCount = new Map<number, number>()
  const incoming = new Map<number, number | null>()
  for (const link of tree.links) {
    childCount.set(link.parent, (childCount.get(link.parent) ?? 0) + 1)
    incoming.set(link.child, link.confidence)
  }
  return [...tree.cells]
    .sort((a, b) => a.t - b.t || a.id - b.id)
    .map((cell) => ({
      cell,
      confidence: incoming.has(cell.id) ? (incoming.get(cell.id) ?? null) : cell.confidence,
      division: (childCount.get(cell.id) ?? 0) >= 2,
      selected: cell.id === selectedId,
    }))
}

export function lineageSpan(rows: readonly LineageRow[]): string {
  const first = rows[0]
  const last = rows[rows.length - 1]
  if (!first || !last) return '—'
  return first.cell.t === last.cell.t ? `T ${first.cell.t}` : `T ${first.cell.t}–${last.cell.t}`
}

export function formatConfidence(value: number | null): string {
  return value === null ? '—' : value.toFixed(2)
}

/** Low confidence is what a proofreader is hunting; it gets the warning color. */
export function isLowConfidence(value: number | null): boolean {
  return value !== null && value < 0.7
}
