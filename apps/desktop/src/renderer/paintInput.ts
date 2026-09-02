import {
  sliceAxis,
  type NavState,
  type PixelZYX,
  type StampPlane,
  type ViewerSession,
  type WorldXYZ,
} from '@cellstudio/viewer'

/** The pointer fields the paint path reads; a `PointerEvent` satisfies it. */
export interface PaintPointerEvent {
  pointerId: number
  button?: number
  clientX: number
  clientY: number
  altKey?: boolean
  shiftKey?: boolean
  preventDefault(): void
  stopPropagation(): void
  stopImmediatePropagation?(): void
}

export interface PaintWheelEvent {
  deltaY: number
  deltaMode?: number
  clientX: number
  clientY: number
  altKey?: boolean
  shiftKey?: boolean
  preventDefault(): void
  stopPropagation(): void
  stopImmediatePropagation?(): void
}

export interface PaintDeps {
  session: ViewerSession
  nav(): NavState
  /** A client point in world units through deck's viewport; null before the first frame. */
  unproject(clientX: number, clientY: number, depth?: number): WorldXYZ | null
  setCapture?(pointerId: number): void
  releaseCapture?(pointerId: number): void
}

export interface PaintHandlers {
  pointerdown(event: PaintPointerEvent): void
  pointermove(event: PaintPointerEvent): void
  pointerup(event: PaintPointerEvent): void
  pointercancel(event: PaintPointerEvent): void
  pointerleave(): void
  wheel(event: PaintWheelEvent): void
  /** `Escape`, window blur, unmount, and any change of frame, tool or view. */
  cancel(): void
  /** Recomputes the 3D ray from the last pointer position after the view moved. */
  refresh(): void
}

/** Wheel pixels per radius step, and the units a non-pixel wheel reports in. */
const RADIUS_WHEEL_PIXELS = 40
const LINE_PIXELS = 16
const PAGE_PIXELS = 400

const isPaintTool = (nav: NavState): boolean => nav.tool === 'brush' || nav.tool === 'eraser'

const wheelPixels = (event: PaintWheelEvent): number =>
  event.deltaY * (event.deltaMode === 1 ? LINE_PIXELS : event.deltaMode === 2 ? PAGE_PIXELS : 1)

const consume = (event: PaintPointerEvent | PaintWheelEvent): void => {
  event.preventDefault()
  event.stopPropagation()
  event.stopImmediatePropagation?.()
}

const direction = (near: WorldXYZ, far: WorldXYZ): WorldXYZ => {
  const d: [number, number, number] = [far[0] - near[0], far[1] - near[1], far[2] - near[2]]
  const length = Math.hypot(d[0], d[1], d[2]) || 1
  return [d[0] / length, d[1] / length, d[2] / length]
}

/**
 * While a paint tool is active the primary drag and the wheel belong to the tool, with
 * `Alt` as the camera modifier. One stroke owns one
 * `pointerId`, and `cancel` is the only other way out of it.
 */
export function paintInput(deps: PaintDeps): PaintHandlers {
  let pointerId: number | null = null
  let radiusAccum = 0
  let last: { x: number; y: number } | null = null

  /** Moves the cursor and returns the voxel a stamp would centre on, or null. */
  const track = (nav: NavState, x: number, y: number): PixelZYX | null => {
    last = { x, y }
    const session = deps.session
    if (nav.activeView === '3d') {
      const near = deps.unproject(x, y, 0)
      const far = deps.unproject(x, y, 1)
      session.volumeScene.setPointerRay(
        near && far ? { origin: near, direction: direction(near, far) } : null,
      )
      const centre = session.volumeScene.orbCentre()
      if (centre && isPaintTool(nav)) {
        // The readout reports the orb, not a hover pixel, while painting in 3D.
        session.readout.move(centre)
      }
      return centre
    }
    const scene = session.slices[nav.activeView]
    const world = deps.unproject(x, y)
    scene.setPointer(world ? [world[0], world[1]] : null)
    return world ? scene.pixelAt([world[0], world[1]]) : null
  }

  const planeOf = (nav: NavState): StampPlane | null =>
    nav.activeView === '3d'
      ? null
      : { axis: sliceAxis(nav.activeView), index: nav.slices[nav.activeView].index }

  /** The selection only targets the stroke while that cell exists on this frame. */
  const selectionOf = (nav: NavState): number | null => {
    const id = nav.selection?.cellId
    if (id === undefined) return null
    return deps.session.tracks.cell(id)?.t === nav.t ? id : null
  }

  const release = (): void => {
    if (pointerId !== null) deps.releaseCapture?.(pointerId)
    pointerId = null
  }

  return {
    pointerdown(event) {
      const nav = deps.nav()
      if (!isPaintTool(nav) || (event.button ?? 0) !== 0 || event.altKey) return
      consume(event)
      const centre = track(nav, event.clientX, event.clientY)
      // A ray that misses the volume paints nothing, and never starts a stroke.
      if (!centre) return
      pointerId = event.pointerId
      deps.setCapture?.(event.pointerId)
      const started = deps.session.editor.begin({
        t: nav.t,
        tool: nav.tool === 'eraser' ? 'eraser' : 'brush',
        radius: nav.brush.radius,
        plane: planeOf(nav),
        centre,
        selection: selectionOf(nav),
      })
      if (!started) release()
    },

    pointermove(event) {
      const nav = deps.nav()
      if (!isPaintTool(nav)) return
      const centre = track(nav, event.clientX, event.clientY)
      if (pointerId === null || event.pointerId !== pointerId) return
      consume(event)
      if (centre) deps.session.editor.move(centre)
    },

    pointerup(event) {
      if (pointerId === null || event.pointerId !== pointerId) return
      consume(event)
      release()
      deps.session.editor.end()
    },

    pointercancel(event) {
      if (pointerId === null || event.pointerId !== pointerId) return
      release()
      deps.session.editor.cancel()
    },

    pointerleave() {
      last = null
      const nav = deps.nav()
      if (nav.activeView === '3d') deps.session.volumeScene.setPointerRay(null)
      else deps.session.slices[nav.activeView].setPointer(null)
    },

    wheel(event) {
      const nav = deps.nav()
      // Alt keeps the wheel on deck's zoom in every view.
      if (!isPaintTool(nav) || event.altKey) return
      const pixels = wheelPixels(event)
      if (event.shiftKey) {
        consume(event)
        radiusAccum += pixels
        const steps = Math.trunc(radiusAccum / RADIUS_WHEEL_PIXELS)
        if (steps === 0) return
        radiusAccum -= steps * RADIUS_WHEEL_PIXELS
        nav.setBrushRadius(nav.brush.radius - steps)
        return
      }
      if (nav.activeView !== '3d') return
      consume(event)
      deps.session.volumeScene.stepOrbU(pixels)
    },

    cancel() {
      release()
      deps.session.editor.cancel()
    },

    refresh() {
      if (!last) return
      track(deps.nav(), last.x, last.y)
    },
  }
}
