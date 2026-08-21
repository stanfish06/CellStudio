import type { LineageTree } from '@cellstudio/api-client'
import { useNav } from '@cellstudio/viewer'
import { formatConfidence, isLowConfidence, lineageRows, lineageSpan } from '../lib/lineage'

export interface LineagePanelProps {
  lineage: LineageTree | null
}

export function LineagePanel({ lineage }: LineagePanelProps) {
  const selection = useNav((s) => s.selection)
  const jumpToCell = useNav((s) => s.jumpToCell)
  const rows = lineageRows(lineage, selection?.cellId ?? null)

  if (rows.length === 0) {
    return (
      <section className="panel">
        <div className="section-title">Lineage</div>
        <p className="empty-note">Select a cell to see its lineage.</p>
      </section>
    )
  }

  return (
    <section className="panel">
      <div className="section-title">
        Lineage of cell {lineage?.rootCellId ?? '—'}
        <span className="minor">{lineageSpan(rows)}</span>
      </div>
      <div className="tree">
        {rows.map((row) => (
          <div className="tree-row" key={row.cell.id}>
            <span className="tree-time">{row.cell.t}</span>
            <i className={row.selected ? 'tree-node selected' : 'tree-node'} />
            <button type="button" className="tree-label" onClick={() => jumpToCell(row.cell)}>
              C{row.cell.id}
              {row.division ? (
                <span>division</span>
              ) : (
                <span className={isLowConfidence(row.confidence) ? 'warn' : undefined}>
                  {formatConfidence(row.confidence)}
                </span>
              )}
            </button>
          </div>
        ))}
      </div>
    </section>
  )
}
