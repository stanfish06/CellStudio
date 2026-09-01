import type { LineageTree } from '@cellstudio/api-client'
import { useNav } from '@cellstudio/viewer'
import { isLowConfidence, lineageLayout, lineageSpan, lineageRows } from '../lib/lineage'
import type { LineageNode } from '../lib/lineage'

export interface LineagePanelProps {
  lineage: LineageTree | null
}

const COL_W = 26
const ROW_H = 20
const PAD = 12
/** Left gutter for the frame labels. */
const GUTTER = 34
const NODE_R = 4.5

const x = (column: number) => GUTTER + PAD + column * COL_W
const y = (row: number) => PAD + row * ROW_H

/** Elbow path: drop to the midpoint, run across, drop into the child. */
const edgePath = (x1: number, y1: number, x2: number, y2: number): string => {
  if (x1 === x2) return `M ${x1} ${y1} L ${x2} ${y2}`
  const ym = y1 + (y2 - y1) / 2
  return `M ${x1} ${y1} L ${x1} ${ym} L ${x2} ${ym} L ${x2} ${y2}`
}

const nodeTitle = (node: LineageNode): string => {
  const parts = [`C${node.cell.id}`, `T ${node.cell.t}`]
  if (node.division) parts.push('division')
  else if (node.confidence !== null) parts.push(`conf ${node.confidence.toFixed(2)}`)
  return parts.join(' · ')
}

/**
 * The selected lineage as a vertical tree: time flows down (y ∝ t), one column per
 * branch, a division is one node with two descending edges. Activating a node jumps
 * to the cell, which also selects it.
 */
export function LineagePanel({ lineage }: LineagePanelProps) {
  const selection = useNav((s) => s.selection)
  const jumpToCell = useNav((s) => s.jumpToCell)
  const layout = lineageLayout(lineage, selection?.cellId ?? null)

  if (!layout) {
    return (
      <section className="panel">
        <div className="section-title">Lineage</div>
        <p className="empty-note">Select a cell to see its lineage.</p>
      </section>
    )
  }

  const byId = new Map(layout.nodes.map((n) => [n.cell.id, n]))
  const width = GUTTER + PAD * 2 + Math.max(1, layout.columns - 1) * COL_W + NODE_R * 2
  const height = PAD * 2 + (layout.tMax - layout.tMin) * ROW_H
  const frames = [...new Set(layout.nodes.map((n) => n.row))]

  return (
    <section className="panel">
      <div className="section-title">
        Lineage of cell {lineage?.rootCellId ?? '—'}
        <span className="minor">{lineageSpan(lineageRows(lineage, null))}</span>
      </div>
      <div className="lineage-scroll">
        <svg
          className="lineage-svg"
          width={width}
          height={height}
          viewBox={`0 0 ${width} ${height}`}
          role="tree"
          aria-label="Lineage tree"
        >
          {frames.map((row) => (
            <text key={row} className="lineage-frame" x={GUTTER - 6} y={y(row) + 3}>
              {layout.tMin + row}
            </text>
          ))}
          {layout.edges.map((edge) => {
            const from = byId.get(edge.parent.id)
            const to = byId.get(edge.child.id)
            if (!from || !to) return null
            return (
              <path
                key={`${edge.parent.id}-${edge.child.id}`}
                className="lineage-edge"
                d={edgePath(x(from.column), y(from.row), x(to.column), y(to.row))}
              />
            )
          })}
          {layout.nodes.map((node) => (
            <circle
              key={node.cell.id}
              className={
                node.selected
                  ? 'lineage-node selected'
                  : isLowConfidence(node.confidence)
                    ? 'lineage-node warn'
                    : 'lineage-node'
              }
              cx={x(node.column)}
              cy={y(node.row)}
              r={node.selected ? NODE_R + 1.5 : NODE_R}
              role="treeitem"
              aria-selected={node.selected}
              aria-label={nodeTitle(node)}
              tabIndex={0}
              onClick={() => jumpToCell(node.cell)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') jumpToCell(node.cell)
              }}
            >
              <title>{nodeTitle(node)}</title>
            </circle>
          ))}
        </svg>
      </div>
    </section>
  )
}
