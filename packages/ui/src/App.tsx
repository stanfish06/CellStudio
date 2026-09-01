import type { CellRow, Histogram, JobState, LineageTree } from '@cellstudio/api-client'
import { useNav } from '@cellstudio/viewer'
import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { Inspector, type InspectorTab } from './inspector/Inspector'
import { isToolEnabled, isTypingTarget, resolveKey } from './lib/keymap'
import { MenuBar } from './shell/MenuBar'
import { Ribbon } from './shell/Ribbon'
import { ShortcutsDialog } from './shell/ShortcutsDialog'
import { StatusBar } from './shell/StatusBar'
import './styles/theme.css'
import './styles/shell.css'
import './styles/view.css'
import './styles/inspector.css'
import type {
  BackendState,
  CursorSample,
  DisplayState,
  HistoryEntry,
  MenuId,
  PerfSample,
  ProjectStatus,
} from './types'
import { ViewPanel } from './view/ViewPanel'

export interface AppProps {
  scene?: ReactNode
  display?: DisplayState
  awaitingFrame?: boolean
  backend?: BackendState
  jobs?: readonly JobState[]
  perf?: PerfSample | null
  cursor?: CursorSample | null
  selection?: CellRow | null
  lineage?: LineageTree | null
  histogram?: Histogram | null
  history?: readonly HistoryEntry[]
  status?: ProjectStatus
  /** Backend or edit failure to show in the status bar. */
  error?: string | null
  /** Non-failure status-bar message — e.g. the written snapshot path. */
  notice?: string | null
  /** True only while `selection` names a cell that exists on the current frame. */
  canDeleteMask?: boolean
  onDeleteMask?: () => void
  /** Unlink the selected track; gated on the nav selection. */
  onUnlink?: () => void
  onUndo?: () => void
  onRedo?: () => void
  onMenu?: (menu: MenuId) => void
  /** File → "Open dataset…" / "Import tracking…". */
  onOpenDataset?: () => void
  onImportTracking?: () => void
  canImportTracking?: boolean
  importTrackingHint?: string
  /** Edit → "Save tracking snapshot". */
  onSaveTrackingSnapshot?: () => void
  canSaveTrackingSnapshot?: boolean
}

const NO_JOBS: readonly JobState[] = []
const NO_HISTORY: readonly HistoryEntry[] = []
const DEFAULT_DISPLAY: DisplayState = { level: 0, zoom: 0 }
const DEFAULT_STATUS: ProjectStatus = { saved: true, pendingWrites: 0 }
const NOOP = () => {}

export function App({
  scene,
  display = DEFAULT_DISPLAY,
  awaitingFrame = false,
  backend = 'starting',
  jobs = NO_JOBS,
  perf = null,
  cursor = null,
  selection = null,
  lineage = null,
  histogram = null,
  history = NO_HISTORY,
  status = DEFAULT_STATUS,
  error = null,
  notice = null,
  canDeleteMask = false,
  onDeleteMask = NOOP,
  onUnlink = NOOP,
  onUndo = NOOP,
  onRedo = NOOP,
  onMenu,
  onOpenDataset,
  onImportTracking,
  canImportTracking = false,
  importTrackingHint,
  onSaveTrackingSnapshot,
  canSaveTrackingSnapshot = false,
}: AppProps) {
  const project = useNav((s) => s.project)
  const tool = useNav((s) => s.tool)
  const setTool = useNav((s) => s.setTool)
  const activeView = useNav((s) => s.activeView)
  const activeChannel = useNav((s) => s.activeChannel)
  const brushRadius = useNav((s) => s.brush.radius)
  const setBrushRadius = useNav((s) => s.setBrushRadius)
  const resetVolumeCamera = useNav((s) => s.resetVolumeCamera)
  const navSelection = useNav((s) => s.selection)
  const navSelectedLink = useNav((s) => s.selectedLink)

  // Arming preconditions. Link needs a graph and a selection; Unlink acts on
  // whichever is selected — one edge cuts that link, a cell deletes its whole track.
  const linkEnabled = (project?.hasGraph ?? false) && navSelection !== null
  const unlinkEnabled = navSelection !== null || navSelectedLink !== null

  const [tab, setTab] = useState<InspectorTab>('inspect')
  const [shortcutsOpen, setShortcutsOpen] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)

  const dismiss = useCallback(() => {
    setShortcutsOpen(false)
    setSettingsOpen(false)
  }, [])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const action = resolveKey(event)
      if (!action) return
      if (isTypingTarget(event.target) && action.kind !== 'dismiss') return

      const nav = useNav.getState()
      switch (action.kind) {
        case 'view':
          nav.setActiveView(action.view)
          break
        case 'stepT':
          nav.stepT(action.delta)
          break
        case 'stepSlice':
          nav.stepSlice(action.delta)
          break
        case 'tool':
          if (isToolEnabled(action.tool)) nav.setTool(action.tool)
          break
        case 'brushRadius':
          nav.setBrushRadius(nav.brush.radius + action.delta)
          break
        case 'deleteMask':
          // Inert without a selection on this frame, matching the ribbon button.
          if (canDeleteMask) onDeleteMask()
          break
        case 'unlink':
          // Inert without a selection, matching the ribbon button.
          if (nav.selection !== null || nav.selectedLink !== null) onUnlink()
          break
        case 'undo':
          onUndo()
          break
        case 'redo':
          onRedo()
          break
        case 'resetView':
          // Only the 3D view shows the pose the key would clear.
          if (nav.activeView === '3d') nav.resetVolumeCamera()
          break
        case 'shortcuts':
          setShortcutsOpen(true)
          break
        case 'dismiss':
          // Esc also disarms a pending link.
          nav.cancelLink()
          dismiss()
          break
      }
      event.preventDefault()
    }

    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [dismiss, canDeleteMask, onDeleteMask, onUnlink, onUndo, onRedo])

  return (
    <div className="app">
      <MenuBar
        onMenu={onMenu}
        onOpenDataset={onOpenDataset}
        onImportTracking={onImportTracking}
        canImportTracking={canImportTracking}
        importTrackingHint={importTrackingHint}
        onSaveTrackingSnapshot={onSaveTrackingSnapshot}
        canSaveTrackingSnapshot={canSaveTrackingSnapshot}
      />
      <Ribbon
        tool={tool}
        onTool={setTool}
        onShortcuts={() => setShortcutsOpen((open) => !open)}
        onResetView={resetVolumeCamera}
        resetEnabled={activeView === '3d'}
        brushRadius={brushRadius}
        onBrushRadius={setBrushRadius}
        onDeleteMask={onDeleteMask}
        deleteEnabled={canDeleteMask}
        linkEnabled={linkEnabled}
        onUnlink={onUnlink}
        unlinkEnabled={unlinkEnabled}
        unlinkTarget={navSelectedLink !== null ? 'edge' : 'track'}
      />
      <main className="workspace">
        <ViewPanel
          scene={scene}
          display={display}
          histogram={histogram}
          dtype={project?.dtype ?? null}
          awaitingFrame={awaitingFrame}
          settingsOpen={settingsOpen}
          onSettingsToggle={() => setSettingsOpen((open) => !open)}
          onSettingsClose={() => setSettingsOpen(false)}
        />
        <Inspector
          tab={tab}
          onTab={setTab}
          selection={selection}
          lineage={lineage}
          history={history}
          jobs={jobs}
          backend={backend}
          status={status}
        />
      </main>
      <StatusBar
        cursor={cursor}
        activeChannel={activeChannel}
        backend={backend}
        jobs={jobs}
        perf={perf}
        pendingWrites={status.pendingWrites}
        error={error}
        notice={notice}
      />
      {shortcutsOpen ? <ShortcutsDialog onClose={() => setShortcutsOpen(false)} /> : null}
    </div>
  )
}
