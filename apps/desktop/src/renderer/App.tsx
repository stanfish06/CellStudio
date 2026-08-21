import { useCallback, useEffect, useState } from 'react'
import { App as Shell, type MenuId } from '@cellstudio/ui'
import { useNav } from '@cellstudio/viewer'
import type { JobState } from '@cellstudio/api-client'
import { useBackend } from './useBackend'
import { useViewerSession } from './useViewerSession'
import { SceneCanvas } from './SceneCanvas'

export function App() {
  const backend = useBackend()
  const { session, status } = useViewerSession(backend.session)
  const initProject = useNav((s) => s.initProject)
  const [jobs, setJobs] = useState<readonly JobState[]>([])

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
      scene={<SceneCanvas session={session} />}
      onMenu={onMenu}
    />
  )
}
