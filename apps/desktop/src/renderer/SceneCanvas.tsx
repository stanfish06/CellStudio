import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import DeckGL from '@deck.gl/react'
import { OrthographicView, type OrbitViewState, type PickingInfo } from '@deck.gl/core'
import { fromWorld, useNav, type ViewerSession } from '@cellstudio/viewer'

const ORTHO = new OrthographicView({ id: 'slice', flipY: true })

const orbitTarget = (state: OrbitViewState): readonly number[] =>
  Array.isArray(state.target) ? state.target : [0, 0, 0]

export const sameOrbitState = (a: OrbitViewState, b: OrbitViewState): boolean => {
  const [ax, ay, az] = orbitTarget(a)
  const [bx, by, bz] = orbitTarget(b)
  return (
    a.zoom === b.zoom &&
    a.rotationX === b.rotationX &&
    a.rotationOrbit === b.rotationOrbit &&
    ax === bx &&
    ay === by &&
    az === bz
  )
}

export const isModified = (event: unknown): boolean => {
  const src = (event as { srcEvent?: MouseEvent } | null)?.srcEvent
  return Boolean(src && (src.altKey || src.ctrlKey || src.metaKey || src.shiftKey))
}

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
  const volumeCamera = useNav((s) => s.volume.camera)
  const setVolumeCamera = useNav((s) => s.setVolumeCamera)

  const layers = useMemo(
    () => scene?.layers() ?? [],
    [scene, tick, t, slices, channels, overlays, axisScale],
  )

  const views = useMemo(
    () => (is3d && session ? session.volumeScene.view() : ORTHO),
    [is3d, session],
  )

  const viewState = useMemo(() => {
    const state = scene?.viewState()
    if (!state) return { target: [0, 0, 0] as [number, number, number], zoom: 0 }
    return state
  }, [scene, tick, t, slices, axisScale, volumeCamera])

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

  // A modifier click on a centroid jumps to XY; deck owns plain and modified double-click.
  const onClick = useCallback(
    (info: PickingInfo, event: unknown) => scene?.handlePick(info, isModified(event)) ?? null,
    [scene],
  )

  return (
    <div
      ref={host}
      style={{ position: 'absolute', inset: '0' }}
      onContextMenu={is3d ? (e) => e.preventDefault() : undefined}
    >
      {session && scene ? (
        <DeckGL
          views={views}
          viewState={{ ...viewState }}
          controller={is3d ? undefined : true}
          layers={layers}
          onHover={onHover}
          onClick={onClick}
          onViewStateChange={({ viewState: next }: { viewState: unknown }) => {
            if (is3d) {
              const orbit = next as OrbitViewState
              if (sameOrbitState(orbit, viewState as OrbitViewState)) return
              setVolumeCamera(session.volumeScene.cameraFrom(orbit))
              return
            }
            const v = next as { target?: number[]; zoom?: number | number[] }
            const zoom = Array.isArray(v.zoom) ? (v.zoom[0] ?? 0) : (v.zoom ?? 0)
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
