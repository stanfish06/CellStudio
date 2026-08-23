import { describe, expect, it, vi } from 'vitest'
import type { OrbitViewState } from '@deck.gl/core'
import type { ActiveView, NavState, Tool, ViewerSession, WorldXYZ } from '@cellstudio/viewer'
import { isModified, sameOrbitState } from './SceneCanvas'
import { paintInput, type PaintPointerEvent, type PaintWheelEvent } from './paintInput'

const orbit = (o: Partial<OrbitViewState> = {}): OrbitViewState => ({
  target: [10, 20, 30],
  zoom: -1.5,
  rotationX: 25,
  rotationOrbit: 40,
  ...o,
})

describe('sameOrbitState', () => {
  it('holds for the pose deck echoes back unchanged', () => {
    expect(sameOrbitState(orbit(), orbit())).toBe(true)
    // A click with no drag emits panStart's anchors only, so the pose is identical.
    expect(sameOrbitState(orbit(), { ...orbit(), target: [10, 20, 30] })).toBe(true)
  })

  it('fails on any moved axis, so a real gesture is never dropped', () => {
    expect(sameOrbitState(orbit(), orbit({ rotationOrbit: 41 }))).toBe(false)
    expect(sameOrbitState(orbit(), orbit({ rotationX: 26 }))).toBe(false)
    expect(sameOrbitState(orbit(), orbit({ zoom: -1.49 }))).toBe(false)
    expect(sameOrbitState(orbit(), orbit({ target: [11, 20, 30] }))).toBe(false)
    expect(sameOrbitState(orbit(), orbit({ target: [10, 21, 30] }))).toBe(false)
    expect(sameOrbitState(orbit(), orbit({ target: [10, 20, 31] }))).toBe(false)
  })
})

describe('isModified', () => {
  it('reports every modifier deck treats as a function key', () => {
    for (const key of ['altKey', 'ctrlKey', 'metaKey', 'shiftKey']) {
      expect(isModified({ srcEvent: { [key]: true } })).toBe(true)
    }
  })

  it('reports a plain click, and tolerates an event with no source', () => {
    expect(isModified({ srcEvent: {} })).toBe(false)
    expect(isModified({})).toBe(false)
    expect(isModified(null)).toBe(false)
    expect(isModified(undefined)).toBe(false)
  })
})

const stub = (o: { tool?: Tool; view?: ActiveView; orb?: WorldXYZ | null } = {}) => {
  const editor = { begin: vi.fn(() => true), move: vi.fn(), end: vi.fn(), cancel: vi.fn() }
  const volumeScene = {
    setPointerRay: vi.fn(),
    orbCentre: vi.fn(() => (o.orb === undefined ? [1, 2, 3] : o.orb)),
    stepOrbU: vi.fn(),
  }
  const slice = { setPointer: vi.fn(), pixelAt: vi.fn(() => [1, 20, 30]) }
  const setBrushRadius = vi.fn()
  const session = {
    editor,
    volumeScene,
    slices: { xy: slice, xz: slice, yz: slice },
    readout: { move: vi.fn() },
    tracks: { cell: () => null },
  }
  const nav = {
    tool: o.tool ?? 'brush',
    activeView: o.view ?? 'xy',
    t: 4,
    activeChannel: 0,
    brush: { radius: 8 },
    overlays: { labels: { on: true, opacity: 0.36 } },
    slices: { xy: { index: 1 }, xz: { index: 2 }, yz: { index: 3 } },
    selection: null,
    setBrushRadius,
  }
  const handlers = paintInput({
    session: session as unknown as ViewerSession,
    nav: () => nav as unknown as NavState,
    unproject: () => [10, 20, 0],
  })
  return { handlers, editor, volumeScene, setBrushRadius }
}

const pointer = (o: Partial<PaintPointerEvent> = {}): PaintPointerEvent => ({
  pointerId: 1,
  button: 0,
  clientX: 100,
  clientY: 120,
  preventDefault: vi.fn(),
  stopPropagation: vi.fn(),
  stopImmediatePropagation: vi.fn(),
  ...o,
})

const wheel = (o: Partial<PaintWheelEvent> = {}): PaintWheelEvent => ({
  deltaY: 80,
  clientX: 100,
  clientY: 120,
  preventDefault: vi.fn(),
  stopPropagation: vi.fn(),
  stopImmediatePropagation: vi.fn(),
  ...o,
})

const consumed = (event: PaintPointerEvent | PaintWheelEvent): boolean =>
  vi.mocked(event.preventDefault).mock.calls.length > 0 &&
  vi.mocked(event.stopPropagation).mock.calls.length > 0

describe('paintInput pointer', () => {
  it('takes the primary drag for the tool, and deck never sees it', () => {
    const { handlers, editor } = stub()
    const down = pointer()
    handlers.pointerdown(down)
    expect(consumed(down)).toBe(true)
    expect(editor.begin).toHaveBeenCalledWith(
      expect.objectContaining({ t: 4, tool: 'brush', radius: 8, plane: { axis: 'z', index: 1 } }),
    )
    const move = pointer()
    handlers.pointermove(move)
    expect(editor.move).toHaveBeenCalledWith([1, 20, 30])
    expect(consumed(move)).toBe(true)
    handlers.pointerup(pointer())
    expect(editor.end).toHaveBeenCalledTimes(1)
  })

  it('leaves an Alt drag to deck, and every gesture of a non-paint tool', () => {
    const { handlers, editor } = stub()
    const alt = pointer({ altKey: true })
    handlers.pointerdown(alt)
    expect(consumed(alt)).toBe(false)
    expect(editor.begin).not.toHaveBeenCalled()

    const pointerTool = stub({ tool: 'pointer' })
    const plain = pointer()
    pointerTool.handlers.pointerdown(plain)
    expect(consumed(plain)).toBe(false)
    expect(pointerTool.editor.begin).not.toHaveBeenCalled()
  })

  it('ignores an event carrying another pointer id', () => {
    const { handlers, editor } = stub()
    handlers.pointerdown(pointer({ pointerId: 1 }))
    handlers.pointermove(pointer({ pointerId: 2 }))
    expect(editor.move).not.toHaveBeenCalled()
    handlers.pointerup(pointer({ pointerId: 2 }))
    expect(editor.end).not.toHaveBeenCalled()
  })

  it('writes nothing when the stroke is cancelled mid-drag', () => {
    const { handlers, editor } = stub()
    handlers.pointerdown(pointer())
    handlers.pointermove(pointer())
    handlers.pointercancel(pointer())
    expect(editor.cancel).toHaveBeenCalledTimes(1)
    expect(editor.end).not.toHaveBeenCalled()
    // The capture is gone, so a later up belongs to nobody.
    handlers.pointerup(pointer())
    expect(editor.end).not.toHaveBeenCalled()
  })

  it('starts no stroke where the 3D ray misses the volume', () => {
    const { handlers, editor, volumeScene } = stub({ view: '3d', orb: null })
    const down = pointer()
    handlers.pointerdown(down)
    expect(volumeScene.setPointerRay).toHaveBeenCalled()
    expect(editor.begin).not.toHaveBeenCalled()
    expect(consumed(down)).toBe(true)
  })
})

describe('paintInput wheel', () => {
  it('drives the orb in 3D and leaves the slice views to deck', () => {
    const three = stub({ view: '3d' })
    const spin = wheel()
    three.handlers.wheel(spin)
    expect(three.volumeScene.stepOrbU).toHaveBeenCalledWith(80)
    expect(consumed(spin)).toBe(true)

    const slice = stub({ view: 'xz' })
    const zoom = wheel()
    slice.handlers.wheel(zoom)
    expect(slice.volumeScene.stepOrbU).not.toHaveBeenCalled()
    expect(consumed(zoom)).toBe(false)
  })

  it('sizes the brush on a shifted wheel, reaching neither the orb nor deck', () => {
    const { handlers, volumeScene, setBrushRadius } = stub({ view: '3d' })
    const small = wheel({ deltaY: -20, shiftKey: true })
    handlers.wheel(small)
    // Under one step of travel: consumed, but the radius has not moved yet.
    expect(consumed(small)).toBe(true)
    expect(setBrushRadius).not.toHaveBeenCalled()
    handlers.wheel(wheel({ deltaY: -60, shiftKey: true }))
    expect(setBrushRadius).toHaveBeenCalledWith(10)
    expect(volumeScene.stepOrbU).not.toHaveBeenCalled()
  })

  it('leaves an Alt wheel to deck in every view', () => {
    const { handlers, volumeScene } = stub({ view: '3d' })
    const alt = wheel({ altKey: true })
    handlers.wheel(alt)
    expect(consumed(alt)).toBe(false)
    expect(volumeScene.stepOrbU).not.toHaveBeenCalled()
  })
})
