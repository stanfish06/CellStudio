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

export interface LineageNode {
  cell: CellRow
  /** Column position in leaf-column units; internal nodes centre over their children. */
  column: number
  /** Vertical position proportional to time: `cell.t − tMin`. */
  row: number
  division: boolean
  selected: boolean
  confidence: number | null
}

export interface LineageEdge {
  parent: CellRow
  child: CellRow
}

export interface LineageLayout {
  nodes: LineageNode[]
  edges: LineageEdge[]
  /** Leaf-column count — the layout's width in column units. */
  columns: number
  tMin: number
  tMax: number
}

/**
 * Vertical tree layout for the Lineage tab: adjacency from `links`, columns assigned
 * post-order by leaf count (a chain keeps its column, a division centres over its two
 * child branches), y proportional to t. Iterative traversal — a lineage is as deep as
 * the movie is long.
 */
export function lineageLayout(
  tree: LineageTree | null,
  selectedId: number | null,
): LineageLayout | null {
  if (!tree || tree.cells.length === 0) return null
  const byId = new Map(tree.cells.map((c) => [c.id, c]))
  const children = new Map<number, number[]>()
  const hasParent = new Set<number>()
  const incoming = new Map<number, number | null>()
  const edges: LineageEdge[] = []
  for (const link of tree.links) {
    const parent = byId.get(link.parent)
    const child = byId.get(link.child)
    if (!parent || !child) continue
    const list = children.get(link.parent)
    if (list) list.push(link.child)
    else children.set(link.parent, [link.child])
    hasParent.add(link.child)
    incoming.set(link.child, link.confidence)
    edges.push({ parent, child })
  }
  const byTime = (a: number, b: number) => {
    const ca = byId.get(a) as CellRow
    const cb = byId.get(b) as CellRow
    return ca.t - cb.t || ca.id - cb.id
  }
  for (const list of children.values()) list.sort(byTime)

  const roots = tree.cells
    .filter((c) => !hasParent.has(c.id))
    .sort((a, b) => a.t - b.t || a.id - b.id)
  const column = new Map<number, number>()
  let nextColumn = 0
  for (const root of roots) {
    // post-order: children first, so a parent can centre over its branches
    const stack: { id: number; expanded: boolean }[] = [{ id: root.id, expanded: false }]
    const onPath = new Set<number>([root.id])
    while (stack.length > 0) {
      const top = stack[stack.length - 1] as { id: number; expanded: boolean }
      if (!top.expanded) {
        top.expanded = true
        for (const child of [...(children.get(top.id) ?? [])].reverse()) {
          if (column.has(child) || onPath.has(child)) continue // cycle or diamond guard
          onPath.add(child)
          stack.push({ id: child, expanded: false })
        }
        continue
      }
      stack.pop()
      const kids = (children.get(top.id) ?? []).filter((c) => column.has(c))
      if (kids.length === 0) {
        column.set(top.id, nextColumn)
        nextColumn += 1
      } else {
        const cols = kids.map((c) => column.get(c) as number)
        column.set(top.id, (Math.min(...cols) + Math.max(...cols)) / 2)
      }
    }
  }

  let tMin = Number.POSITIVE_INFINITY
  let tMax = Number.NEGATIVE_INFINITY
  for (const cell of tree.cells) {
    if (cell.t < tMin) tMin = cell.t
    if (cell.t > tMax) tMax = cell.t
  }
  const nodes: LineageNode[] = [...tree.cells]
    .sort((a, b) => a.t - b.t || a.id - b.id)
    .map((cell) => ({
      cell,
      column: column.get(cell.id) ?? 0,
      row: cell.t - tMin,
      division: (children.get(cell.id)?.length ?? 0) >= 2,
      selected: cell.id === selectedId,
      confidence: incoming.has(cell.id) ? (incoming.get(cell.id) ?? null) : cell.confidence,
    }))
  return { nodes, edges, columns: Math.max(1, nextColumn), tMin, tMax }
}
