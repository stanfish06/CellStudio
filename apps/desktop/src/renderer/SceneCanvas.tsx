import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import DeckGL, { type DeckGLRef } from '@deck.gl/react'
import { OrthographicView, type OrbitViewState, type PickingInfo } from '@deck.gl/core'
import { fromWorld, useNav, type ViewerSession, type WorldXYZ } from '@cellstudio/viewer'
import { paintInput, type PaintHandlers } from './paintInput'

const ORTHO = new OrthographicView({ id: 'slice', flipY: true })

/** Capture phase, and cancellable: React's synthetic capture fires at the root, while
 * mjolnir listens natively on the deck canvas below this element (design M13). */
const CAPTURE: AddEventListenerOptions = { capture: true, passive: false }

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
  const deck = useRef<DeckGLRef | null>(null)
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

  const paint = useMemo<PaintHandlers | null>(() => {
    if (!session) return null
    return paintInput({
      session,
      nav: () => useNav.getState(),
      unproject: (clientX, clientY, depth) => {
        const box = host.current?.getBoundingClientRect()
        const viewport = deck.current?.deck?.getViewports()[0]
        if (!box || !viewport) return null
        const point = viewport.unproject([clientX - box.left, clientY - box.top, depth ?? 0])
        return [point[0] ?? 0, point[1] ?? 0, point[2] ?? 0] as WorldXYZ
      },
      setCapture: (id) => host.current?.setPointerCapture(id),
      releaseCapture: (id) => {
        if (host.current?.hasPointerCapture(id)) host.current.releasePointerCapture(id)
      },
    })
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
      // The ray depends on the projection, so a resize moves the orb's interval.
      paint?.refresh()
      setTick((n) => n + 1)
    })
    observer.observe(el)
    return () => observer.disconnect()
  }, [session, paint])

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
  const tool = useNav((s) => s.tool)
  const sliceIndex = slices[activeView === '3d' ? 'xy' : activeView].index

  useEffect(() => {
    const el = host.current
    if (!el || !paint) return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') paint.cancel()
    }
    el.addEventListener('pointerdown', paint.pointerdown, CAPTURE)
    el.addEventListener('pointermove', paint.pointermove, CAPTURE)
    el.addEventListener('pointerup', paint.pointerup, CAPTURE)
    el.addEventListener('pointercancel', paint.pointercancel, CAPTURE)
    el.addEventListener('lostpointercapture', paint.pointercancel, CAPTURE)
    el.addEventListener('pointerleave', paint.pointerleave, CAPTURE)
    el.addEventListener('wheel', paint.wheel, CAPTURE)
    window.addEventListener('blur', paint.cancel)
    window.addEventListener('keydown', onKeyDown)
    return () => {
      el.removeEventListener('pointerdown', paint.pointerdown, CAPTURE)
      el.removeEventListener('pointermove', paint.pointermove, CAPTURE)
      el.removeEventListener('pointerup', paint.pointerup, CAPTURE)
      el.removeEventListener('pointercancel', paint.pointercancel, CAPTURE)
      el.removeEventListener('lostpointercapture', paint.pointercancel, CAPTURE)
      el.removeEventListener('pointerleave', paint.pointerleave, CAPTURE)
      el.removeEventListener('wheel', paint.wheel, CAPTURE)
      window.removeEventListener('blur', paint.cancel)
      window.removeEventListener('keydown', onKeyDown)
      paint.cancel()
    }
  }, [paint])

  // A stroke is bound to one frame, tool and view; any of them moving ends it (design M4).
  useEffect(() => {
    paint?.cancel()
  }, [paint, t, tool, activeView, sliceIndex])

  // The orb keeps its relative depth when the view moves, so the interval is recomputed.
  useEffect(() => {
    paint?.refresh()
  }, [paint, volumeCamera, axisScale, activeView])

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
          ref={deck}
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
