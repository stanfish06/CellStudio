import { useCallback, useEffect, useRef, useState } from 'react'
import {
  ApiClient,
  EventStream,
  makeStore,
  type ProjectInfo,
  type Versions,
} from '@cellstudio/api-client'
import type { BackendState, CellStudioBridge } from '../shared/bridge'

declare global {
  interface Window {
    cellstudio: CellStudioBridge
  }
}

export interface BackendSession {
  api: ApiClient
  events: EventStream
  store: ReturnType<typeof makeStore>
  generation: number
}

export interface BackendHandle {
  state: BackendState
  session: BackendSession | null
  project: ProjectInfo | null
  versions: Versions | null
  error: string | null
  openDataset(): Promise<void>
}

const LAST_PROJECT_KEY = 'cellstudio.lastProjectPath'

export function useBackend(): BackendHandle {
  const [state, setState] = useState<BackendState>('starting')
  const [session, setSession] = useState<BackendSession | null>(null)
  const [project, setProject] = useState<ProjectInfo | null>(null)
  const [versions, setVersions] = useState<Versions | null>(null)
  const [error, setError] = useState<string | null>(null)
  const sessionRef = useRef<BackendSession | null>(null)

  const teardown = useCallback(() => {
    sessionRef.current?.events.close()
    sessionRef.current = null
    setSession(null)
    setProject(null)
    setVersions(null)
  }, [])

  const rebuild = useCallback(async () => {
    teardown()
    const info = await window.cellstudio.getBackendInfo()
    if (!info) return

    const api = new ApiClient({ baseUrl: info.baseUrl, token: info.token })
    const events = new EventStream(api)
    events.on('versions', (e) => setVersions(e.versions))
    events.connect()

    const next = {
      api,
      events,
      store: makeStore(info.baseUrl, info.token),
      generation: info.generation,
    }
    sessionRef.current = next
    setSession(next)
    setError(null)

    // restore the previous project
    const previous =
      (await window.cellstudio.openOnStart()) ?? localStorage.getItem(LAST_PROJECT_KEY)
    if (!previous) {
      const current = await api.getProject()
      if (current) setProject(current)
      return
    }
    try {
      setProject(await api.openProject(previous))
    } catch (e) {
      setError(
        `Could not reopen ${previous}: ${e instanceof Error ? e.message : String(e)}. ` +
          'Another CellStudio window may have it open.',
      )
    }
  }, [teardown])

  useEffect(() => {
    const dispose = window.cellstudio.onBackendState(({ state: next }) => {
      setState(next)
      if (next === 'ready') void rebuild()
      if (next === 'down' || next === 'fatal') teardown()
      if (next === 'fatal') setError('The analysis backend could not be started.')
    })
    void window.cellstudio.getBackendInfo().then((info) => {
      if (info) {
        setState('ready')
        void rebuild()
      }
    })
    return dispose
  }, [rebuild, teardown])

  const openDataset = useCallback(async () => {
    const path = await window.cellstudio.openDatasetDialog()
    if (!path || !sessionRef.current) return
    try {
      const info = await sessionRef.current.api.openProject(path)
      localStorage.setItem(LAST_PROJECT_KEY, path)
      setProject(info)
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  return { state, session, project, versions, error, openDataset }
}
