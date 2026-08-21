import type { CellRow, JobState, LineageTree } from '@cellstudio/api-client'
import type { BackendState, HistoryEntry, ProjectStatus } from '../types'
import { HistoryPanel } from './HistoryPanel'
import { InspectPanel } from './InspectPanel'
import { LineagePanel } from './LineagePanel'

export type InspectorTab = 'inspect' | 'lineage' | 'history'

const TABS: readonly { id: InspectorTab; label: string }[] = [
  { id: 'inspect', label: 'Inspect' },
  { id: 'lineage', label: 'Lineage' },
  { id: 'history', label: 'History' },
]

export interface InspectorProps {
  tab: InspectorTab
  onTab: (tab: InspectorTab) => void
  selection: CellRow | null
  lineage: LineageTree | null
  history: readonly HistoryEntry[]
  jobs: readonly JobState[]
  backend: BackendState
  status: ProjectStatus
}

export function Inspector({
  tab,
  onTab,
  selection,
  lineage,
  history,
  jobs,
  backend,
  status,
}: InspectorProps) {
  return (
    <aside className="inspector" aria-label="Context panel">
      <div className="tabs" role="tablist">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={t.id === tab}
            className={t.id === tab ? 'tab active' : 'tab'}
            onClick={() => onTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div className="panel-scroll" role="tabpanel">
        {tab === 'inspect' ? (
          <InspectPanel selection={selection} jobs={jobs} backend={backend} status={status} />
        ) : null}
        {tab === 'lineage' ? <LineagePanel lineage={lineage} /> : null}
        {tab === 'history' ? <HistoryPanel entries={history} /> : null}
      </div>
    </aside>
  )
}
