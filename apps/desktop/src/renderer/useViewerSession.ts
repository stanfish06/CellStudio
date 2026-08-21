import { useEffect, useMemo, useState, useSyncExternalStore } from 'react'
import { ViewerSession, useNav, type SceneStatus } from '@cellstudio/viewer'
import type { BackendSession } from './useBackend'

const IDLE: SceneStatus = { display: { level: 0, zoom: 0 }, awaitingFrame: false }

/**
 * Binds the viewer's scene session to the backend session: a new backend generation
 * builds a new ViewerSession, and the old one's caches die with it (design D16).
 */
export function useViewerSession(backend: BackendSession | null): {
  session: ViewerSession | null
  status: SceneStatus
} {
  const [session, setSession] = useState<ViewerSession | null>(null)

  useEffect(() => {
    if (!backend) {
      setSession(null)
      return
    }
    const next = new ViewerSession({
      api: backend.api,
      store: backend.store,
      nav: useNav.getState(),
    })
    setSession(next)
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

  const subscribe = useMemo(
    () => (cb: () => void) => (session ? session.onChange(cb) : () => {}),
    [session],
  )
  const status = useSyncExternalStore(
    subscribe,
    () => session?.status ?? IDLE,
    () => IDLE,
  )

  return { session, status }
}
