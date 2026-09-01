import type { HistoryEntry } from '../types'

export interface HistoryPanelProps {
  entries: readonly HistoryEntry[]
  enabled?: boolean
  onUndo?: () => void
  onRedo?: () => void
}

export function HistoryPanel({ entries, enabled = false, onUndo, onRedo }: HistoryPanelProps) {
  return (
    <section className="panel">
      <div className="section-title">
        Edit history
        <span className="minor">
          {entries.length} {entries.length === 1 ? 'action' : 'actions'}
        </span>
      </div>
      <div className="history-actions">
        <button type="button" className="small-button primary" disabled={!enabled} onClick={onUndo}>
          Undo
        </button>
        <button type="button" className="small-button" disabled={!enabled} onClick={onRedo}>
          Redo
        </button>
      </div>
      {entries.length === 0 ? (
        <p className="empty-note">
          No edits yet — mask and track editing ship after the viewing phase.
        </p>
      ) : (
        <div className="history-list">
          {entries.map((entry) => (
            <div className={entry.undone ? 'history-item undone' : 'history-item'} key={entry.seq}>
              <div className="history-head">
                <span>{entry.summary}</span>
                <span className="history-seq">{entry.undone ? 'UNDONE' : `#${entry.seq}`}</span>
              </div>
              <div className="history-meta">
                <span className="domain-tag">{entry.domain}</span>
                {entry.scope} · {entry.time}
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  )
}
