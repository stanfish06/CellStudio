import { useCallback, useEffect, useMemo, useState } from 'react'
import { App as Shell, type HistoryEntry, type MenuId } from '@cellstudio/ui'
import { useNav } from '@cellstudio/viewer'
import type { EditEntry, JobState, LineageTree } from '@cellstudio/api-client'
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
  const hasGraph = useNav((s) => s.project?.hasGraph ?? false)
  const t = useNav((s) => s.t)
  const [jobs, setJobs] = useState<readonly JobState[]>([])
  const [history, setHistory] = useState<readonly HistoryEntry[]>([])
  const [lineage, setLineage] = useState<LineageTree | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

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
    const dispose = session.events.on('job', (event) => {
      refresh()
      // The snapshot job's completion message carries the written path.; the
      // import job's carries its counts.
      const { kind, status, message } = event.job
      if (kind !== 'export' && kind !== 'import-tracks') return
      const what = kind === 'export' ? 'Snapshot' : 'Tracking import'
      if (status === 'done') setNotice(message ?? `${what} finished`)
      if (status === 'failed') setNotice(`${what} failed: ${message ?? 'unknown error'}`)
      if (status === 'cancelled') setNotice(`${what} cancelled: ${message ?? 'session replaced'}`)
    })
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

  // The lineage of the selection: fetched on selection change and after every applied
  // graph advance, latest-wins so a stale response never lands.
  useEffect(() => {
    const api = backend.session?.api
    if (!api || !session || selectionId === null) {
      setLineage(null)
      session?.setLineage(null)
      return
    }
    let live = true
    let seq = 0
    const fetchLineage = (graphVersion: number) => {
      const mine = ++seq
      api.lineage(selectionId).then(
        (tree) => {
          if (!live || mine !== seq) return
          setLineage(tree)
          session.setLineage({ ...tree, graphVersion })
        },
        () => {
          if (!live || mine !== seq) return
          setLineage(null)
          session.setLineage(null)
        },
      )
    }
    fetchLineage(useNav.getState().project?.versions.graph ?? 0)
    const off = session.onGraphAdvance((graphVersion) => fetchLineage(graphVersion))
    return () => {
      live = false
      off()
    }
  }, [backend.session, session, selectionId])

  // hasGraph freshness: the first graph edit and a finished import both land here through
  // advanceGraph, so Link/Unlink/save enable without a /project refetch.
  useEffect(() => {
    if (!session) return
    return session.onGraphAdvance(() => useNav.getState().markGraphPresent())
  }, [session])

  const selection = selectionId === null ? null : (session?.tracks.cell(selectionId) ?? null)
  const canDeleteMask = selection !== null && selection.t === t

  const onDeleteMask = useCallback(() => {
    if (!session || selectionId === null || !canDeleteMask) return
    session.editor.deleteMask(t, selectionId)
    useNav.getState().select(null)
  }, [session, selectionId, canDeleteMask, t])

  // one selected edge cuts that link; a selected cell deletes its whole track
  const onUnlink = useCallback(() => {
    if (!session) return
    const edge = useNav.getState().selectedLink
    if (edge) session.cutLink(edge.parent, edge.child)
    else if (selectionId !== null) session.unlinkCell(selectionId)
  }, [session, selectionId])

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

  const onOpenDataset = useCallback(() => void backend.openDataset(), [backend])

  const onImportTracking = useCallback(() => {
    const api = backend.session?.api
    if (!api) return
    void (async () => {
      const path = await window.cellstudio.openTrackingDialog()
      if (!path) return
      setNotice('Importing tracking…')
      try {
        await api.startImport('tracks', path)
      } catch (e) {
        setNotice(`Tracking import failed: ${e instanceof Error ? e.message : String(e)}`)
      }
    })()
  }, [backend.session])

  const onSaveTrackingSnapshot = useCallback(() => {
    const api = backend.session?.api
    if (!api) return
    setNotice(null)
    void api.exportTracks().catch((e) => setNotice(e instanceof Error ? e.message : String(e)))
  }, [backend.session])

  return (
    <Shell
      backend={backend.state}
      jobs={jobs}
      display={status.display}
      awaitingFrame={status.awaitingFrame}
      cursor={session?.readout.sample ?? null}
      selection={selection}
      lineage={lineage}
      history={history}
      status={projectStatus}
      error={editError ?? backend.error}
      notice={notice}
      canDeleteMask={canDeleteMask}
      onDeleteMask={onDeleteMask}
      onUnlink={onUnlink}
      onUndo={onUndo}
      onRedo={onRedo}
      onOpenDataset={onOpenDataset}
      onImportTracking={onImportTracking}
      canImportTracking={backend.project !== null && !hasGraph}
      importTrackingHint={
        backend.project === null ? 'Open a dataset first' : 'This project already has a track graph'
      }
      onSaveTrackingSnapshot={onSaveTrackingSnapshot}
      canSaveTrackingSnapshot={hasGraph}
      scene={<SceneCanvas session={session} />}
      onMenu={onMenu}
    />
  )
}
