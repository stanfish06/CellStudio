import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import DeckGL from '@deck.gl/react'
import { OrbitView, OrthographicView, type PickingInfo } from '@deck.gl/core'
import { fromWorld, useNav, type ViewerSession } from '@cellstudio/viewer'

const ORTHO = new OrthographicView({ id: 'slice', flipY: true })
const ORBIT = new OrbitView({ id: 'volume' })

export function SceneCanvas({ session }: { session: ViewerSession | null }) {
  const host = useRef<HTMLDivElement | null>(null)
  const activeView = useNav((s) => s.activeView)
  const t = useNav((s) => s.t)
  const slices = useNav((s) => s.slices)
  const channels = useNav((s) => s.channels)
  const overlays = useNav((s) => s.overlays)
  const axisScale = useNav((s) => s.axisScale)
  const [tick, setTick] = useState(0)

  useEffect(() => {
    if (!session) return
    return session.onChange(() => setTick((n) => n + 1))
  }, [session])

  useEffect(() => {
    const el = host.current
    if (!el || !session) return
    const observer = new ResizeObserver(([entry]) => {
      const box = entry?.contentRect
      if (!box || box.width === 0 || box.height === 0) return
      const viewport = { width: box.width, height: box.height }
      for (const orientation of ['xy', 'xz', 'yz'] as const) {
        session.slices[orientation].setViewport(viewport)
      }
      session.volumeScene.setViewport(viewport)
      setTick((n) => n + 1)
    })
    observer.observe(el)
    return () => observer.disconnect()
  }, [session])

  const scene = session?.scene(activeView) ?? null
  const is3d = activeView === '3d'

  const layers = useMemo(
    () => scene?.layers() ?? [],
    [scene, tick, t, slices, channels, overlays, axisScale],
  )

  const viewState = useMemo(() => {
    const state = scene?.viewState()
    if (!state) return { target: [0, 0, 0] as [number, number, number], zoom: 0 }
    return state
  }, [scene, tick, t, slices, axisScale])

  const activeChannel = useNav((s) => s.activeChannel)
  const sliceIndex = slices[activeView === '3d' ? 'xy' : activeView].index

  const onHover = useCallback(
    (info: PickingInfo) => {
      if (!session || is3d || !info.coordinate) return
      const [wx, wy] = info.coordinate
      const scale = useNav.getState().project?.scale ?? null
      const [, py, px] = fromWorld([wx ?? 0, wy ?? 0, 0], scale, axisScale)
      session.readout.move([sliceIndex, py, px], {
        t,
        channel: activeChannel,
        labels: overlays.labels.on,
      })
    },
    [session, is3d, sliceIndex, t, activeChannel, overlays.labels.on, axisScale],
  )

  const onClick = useCallback((info: PickingInfo) => scene?.handlePick(info) ?? null, [scene])

  return (
    <div ref={host} style={{ position: 'absolute', inset: '0' }}>
      {session && scene ? (
        <DeckGL
          views={is3d ? ORBIT : ORTHO}
          viewState={{ ...viewState }}
          controller
          layers={layers}
          onHover={onHover}
          onClick={onClick}
          onViewStateChange={({ viewState: next }: { viewState: unknown }) => {
            const v = next as { target?: number[]; zoom?: number | number[] }
            const zoom = Array.isArray(v.zoom) ? (v.zoom[0] ?? 0) : (v.zoom ?? 0)
            if (is3d) return
            const target = v.target ?? [0, 0]
            session.slices[activeView as 'xy' | 'xz' | 'yz'].setCamera({
              target: [target[0] ?? 0, target[1] ?? 0],
              zoom,
            })
          }}
          style={{ position: 'absolute', inset: '0' }}
        />
      ) : null}
    </div>
  )
}
