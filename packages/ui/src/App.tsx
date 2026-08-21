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
  onMenu?: (menu: MenuId) => void
}

const NO_JOBS: readonly JobState[] = []
const NO_HISTORY: readonly HistoryEntry[] = []
const DEFAULT_DISPLAY: DisplayState = { level: 0, zoom: 0 }
const DEFAULT_STATUS: ProjectStatus = { saved: true, pendingWrites: 0 }

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
  onMenu,
}: AppProps) {
  const project = useNav((s) => s.project)
  const tool = useNav((s) => s.tool)
  const setTool = useNav((s) => s.setTool)
  const activeView = useNav((s) => s.activeView)
  const activeChannel = useNav((s) => s.activeChannel)
  const resetVolumeCamera = useNav((s) => s.resetVolumeCamera)

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
        case 'resetView':
          // Only the 3D view shows the pose the key would clear.
          if (nav.activeView === '3d') nav.resetVolumeCamera()
          break
        case 'shortcuts':
          setShortcutsOpen(true)
          break
        case 'dismiss':
          dismiss()
          break
      }
      event.preventDefault()
    }

    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [dismiss])

  return (
    <div className="app">
      <MenuBar onMenu={onMenu} />
      <Ribbon
        tool={tool}
        onTool={setTool}
        onShortcuts={() => setShortcutsOpen((open) => !open)}
        onResetView={resetVolumeCamera}
        resetEnabled={activeView === '3d'}
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
      />
      {shortcutsOpen ? <ShortcutsDialog onClose={() => setShortcutsOpen(false)} /> : null}
    </div>
  )
}
