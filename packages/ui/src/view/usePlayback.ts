import { sliceAxis, useNav } from '@cellstudio/viewer'
import { useEffect, useRef } from 'react'
import { nextPlaybackIndex } from '../lib/transport'

/** ~15 steps/s ceiling, above the 10 fps XY playback floor; the gate below slows it to what presents. */
export const PLAYBACK_INTERVAL_MS = 66

/**
 * Animates the playing axis. Each step is scheduled after the previous one commits and is
 * skipped while the active view still owes a frame, so requests never queue behind the user
 * (spec: "advancing only as data presents").
 */
export function usePlayback(awaitingFrame: boolean, intervalMs = PLAYBACK_INTERVAL_MS): void {
  const playing = useNav((s) => s.transport.playing)
  const activeView = useNav((s) => s.activeView)
  const setPlaying = useNav((s) => s.setPlaying)
  const awaiting = useRef(awaitingFrame)
  awaiting.current = awaitingFrame

  // The 3D view has no slice axis, so switching into it ends slice playback.
  useEffect(() => {
    if (playing === 'slice' && activeView === '3d') setPlaying('off')
  }, [playing, activeView, setPlaying])

  useEffect(() => {
    if (playing === 'off') return
    let timer: ReturnType<typeof setTimeout>

    const tick = () => {
      timer = setTimeout(() => {
        if (!awaiting.current) advance(playing)
        tick()
      }, intervalMs)
    }
    tick()

    return () => clearTimeout(timer)
  }, [playing, intervalMs])
}

// Read through the store rather than a render closure so a long playback never steps stale state.
function advance(axis: 't' | 'slice'): void {
  const nav = useNav.getState()
  const dims = nav.project?.dims
  if (!dims) return
  if (axis === 't') {
    nav.setT(nextPlaybackIndex(nav.t, dims.t - 1))
    return
  }
  if (nav.activeView === '3d') return
  const max = dims[sliceAxis(nav.activeView)] - 1
  nav.setSliceIndex(nextPlaybackIndex(nav.slices[nav.activeView].index, max))
}
