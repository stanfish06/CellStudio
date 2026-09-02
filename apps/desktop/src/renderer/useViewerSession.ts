import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { type CursorSample, type SceneStatus, ViewerSession, useNav } from '@cellstudio/viewer'
import type { BackendSession } from './useBackend'

const IDLE: SceneStatus = { display: { level: 0, zoom: 0 }, awaitingFrame: false }

const message = (error: unknown): string => (error instanceof Error ? error.message : String(error))

/**
 * Binds the viewer's scene session to the backend session: a new backend generation
 * builds a new ViewerSession, and the old one's caches die with it. */
export function useViewerSession(backend: BackendSession | null): {
  session: ViewerSession | null
  status: SceneStatus
  pendingWrites: number
  editError: string | null
  cursor: CursorSample | null
} {
  const [session, setSession] = useState<ViewerSession | null>(null)
  const [editError, setEditError] = useState<string | null>(null)

  useEffect(() => {
    if (!backend) {
      setSession(null)
      return
    }
    const next = new ViewerSession({
      api: backend.api,
      store: backend.store,
      nav: useNav.getState(),
      onEditError: (error) => setEditError(message(error)),
    })
    setSession(next)
    setEditError(null)
    return () => {
      next.dispose()
      setSession(null)
    }
  }, [backend])

  // The store is the source of navigation truth; the scene session follows it.
  useEffect(() => {
    if (!session) return
    session.update(useNav.getState())
    return useNav.subscribe((state) => session.update(state))
  }, [session])

  /**
   * Server pushes reach the pixels here, not in `useBackend`, which owns the stream but
   * holds no `ViewerSession`. Both a mask response and this event run one `advanceLabels`,
   * which is idempotent by version.   */
  useEffect(() => {
    if (!session || !backend) return
    const disposers = [
      backend.events.on('invalidate', (event) => {
        if (event.layer === 'labels') session.advanceLabels(event.sessionId, event.version)
        else session.invalidate(event.layer, event.version)
      }),
      // Both the WS event and the HTTP edit result run one advanceGraph, idempotent by
      // version — same shape as the mask path.
      backend.events.on('graphChanged', (event) =>
        session.advanceGraph(event.sessionId, event.graphVersion, { tracks: event.tracks }),
      ),
      // Reconnect resyncs versions rather than invalidations, so the version comparison is
      // what recovers an edit that committed while the socket was down.
      backend.events.on('versions', (event) => {
        session.advanceLabels(event.versions.sessionId, event.versions.labels)
        session.advanceGraph(event.versions.sessionId, event.versions.graph)
      }),
    ]
    return () => {
      for (const dispose of disposers) dispose()
    }
  }, [session, backend])

  const subscribe = useMemo(
    () => (cb: () => void) => (session ? session.onChange(cb) : () => {}),
    [session],
  )
  const status = useSyncExternalStore(
    subscribe,
    () => session?.status ?? IDLE,
    () => IDLE,
  )
  const pendingWrites = useSyncExternalStore(
    subscribe,
    () => session?.pendingWrites ?? 0,
    () => 0,
  )
  const cursor = useSyncExternalStore(
    subscribe,
    () => session?.readout.sample ?? null,
    () => null,
  )

  // A newly queued edit makes the last failure stale.
  const wasPending = useRef(0)
  useEffect(() => {
    if (pendingWrites > wasPending.current) setEditError(null)
    wasPending.current = pendingWrites
  }, [pendingWrites])

  return { session, status, pendingWrites, editError, cursor }
}
