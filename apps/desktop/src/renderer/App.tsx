import { useCallback, useEffect, useMemo, useState } from 'react'
import { App as Shell, type HistoryEntry, type MenuId } from '@cellstudio/ui'
import { useNav } from '@cellstudio/viewer'
import type { EditEntry, JobState } from '@cellstudio/api-client'
import { useBackend } from './useBackend'
import { useViewerSession } from './useViewerSession'
import { SceneCanvas } from './SceneCanvas'

/** `GET /edits` rows as the History tab renders them; `undoable` is the pruned-blob flag. */
const historyEntry = (entry: EditEntry): HistoryEntry => ({
  seq: entry.seq,
  domain: entry.domain,
  summary: entry.scope ?? (entry.domain === 'mask' ? 'Mask edit' : 'Graph edit'),
  scope: entry.undoable ? 'undoable' : 'beyond the undo window',
  time: new Date(entry.ts).toLocaleTimeString(),
  undone: entry.undone,
})

export function App() {
  const backend = useBackend()
  const { session, status, pendingWrites, editError } = useViewerSession(backend.session)
  const initProject = useNav((s) => s.initProject)
  const selectionId = useNav((s) => s.selection?.cellId ?? null)
  const t = useNav((s) => s.t)
  const [jobs, setJobs] = useState<readonly JobState[]>([])
  const [history, setHistory] = useState<readonly HistoryEntry[]>([])

  useEffect(() => {
    if (backend.project) initProject(backend.project)
    const name = backend.project?.projectPath.split('/').pop()
    document.title = name ? `${name}` : ''
  }, [backend.project, initProject])

  useEffect(() => {
    const session = backend.session
    if (!session) {
      setJobs([])
      return
    }
    let live = true
    const refresh = () => {
      void session.api.jobs().then((next) => {
        if (live) setJobs(next)
      })
    }
    refresh()
    const dispose = session.events.on('job', refresh)
    return () => {
      live = false
      dispose()
    }
  }, [backend.session])

  // Every edit bumps a version, which is what makes the journal worth re-reading.
  const editVersions = `${backend.versions?.labels ?? 0}/${backend.versions?.graph ?? 0}`
  useEffect(() => {
    const api = backend.session?.api
    if (!api) {
      setHistory([])
      return
    }
    let live = true
    void api
      .edits()
      .then((rows) => {
        if (live) setHistory(rows.map(historyEntry))
      })
      .catch(() => {})
    return () => {
      live = false
    }
  }, [backend.session, editVersions])

  const selection = selectionId === null ? null : (session?.tracks.cell(selectionId) ?? null)
  const canDeleteMask = selection !== null && selection.t === t

  const onDeleteMask = useCallback(() => {
    if (!session || selectionId === null || !canDeleteMask) return
    session.editor.deleteMask(t, selectionId)
    useNav.getState().select(null)
  }, [session, selectionId, canDeleteMask, t])

  const onUndo = useCallback(() => session?.editor.undo(), [session])
  const onRedo = useCallback(() => session?.editor.redo(), [session])

  const projectStatus = useMemo(
    () => ({ saved: pendingWrites === 0, pendingWrites }),
    [pendingWrites],
  )

  const onMenu = useCallback(
    (menu: MenuId) => {
      if (menu === 'file') void backend.openDataset()
    },
    [backend],
  )

  return (
    <Shell
      backend={backend.state}
      jobs={jobs}
      display={status.display}
      awaitingFrame={status.awaitingFrame}
      cursor={session?.readout.sample ?? null}
      selection={selection}
      history={history}
      status={projectStatus}
      error={editError ?? backend.error}
      canDeleteMask={canDeleteMask}
      onDeleteMask={onDeleteMask}
      onUndo={onUndo}
      onRedo={onRedo}
      scene={<SceneCanvas session={session} />}
      onMenu={onMenu}
    />
  )
}
