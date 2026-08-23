import { afterEach, describe, expect, it, vi } from 'vitest'
import type { Bbox, CellRow } from '@cellstudio/api-client'
import { ViewerSession } from './session'
import { GpuBudget } from '../data/gpuBudget'
import type { NavSnapshot } from './types'
import type { OrbitCamera } from '../state/nav'
import { FakeApi, cell, devProject, navSnapshot } from '../test/data'

const settle = () => new Promise((r) => setTimeout(r, 0))

const pose = (zoom: number): OrbitCamera => ({
  rotationX: 30,
  rotationOrbit: 45,
  zoom,
  target: [1, 2, 3],
})

const moved = (nav: NavSnapshot, zoom: number): NavSnapshot => ({
  ...nav,
  volume: { camera: pose(zoom) },
})

/** `FakeApi` answers `cellsWindow` immediately; this one holds the window open. */
class HeldCellsApi extends FakeApi {
  private pendingCells: ((rows: CellRow[]) => void)[] = []

  override cellsWindow(q: { t0: number; t1: number; bbox?: Bbox }): Promise<CellRow[]> {
    this.cellCalls.push({ t0: q.t0, t1: q.t1 })
    return new Promise<CellRow[]>((resolve) => {
      this.pendingCells.push(resolve)
    })
  }

  get openCells(): number {
    return this.pendingCells.length
  }
}

const session = (api: FakeApi, tSettleMs = 150) =>
  new ViewerSession({
    api,
    budget: new GpuBudget({ totalBytes: 512 * 1024 * 1024, volumeCeilingBytes: 8 * 1024 * 1024 }),
    tSettleMs,
    nav: { select: () => {}, jumpToCell: () => {} },
  })

describe('ViewerSession', () => {
  const project = devProject()
  afterEach(() => vi.useRealTimers())

  it('drives only the active view', async () => {
    const api = new FakeApi()
    const s = session(api)
    s.update(navSnapshot(project, { activeView: 'xz' }))
    await settle()
    expect(api.sliceCalls.every((c) => c.q.axis === 'xz')).toBe(true)
    expect(api.volumeCalls).toHaveLength(0)
    s.dispose()
  })

  it('warms inactive views once t settles, not on every step', async () => {
    vi.useFakeTimers()
    const api = new FakeApi()
    const s = session(api)
    for (let t = 0; t < 10; t += 1) {
      s.update(navSnapshot(project, { activeView: 'xy', t, generation: t + 1 }))
      await vi.advanceTimersByTimeAsync(20)
    }
    expect(api.sliceCalls).toHaveLength(0)
    expect(api.volumeCalls).toHaveLength(0)
    await vi.advanceTimersByTimeAsync(160)
    expect(new Set(api.sliceCalls.map((c) => c.q.axis))).toEqual(new Set(['xz', 'yz']))
    expect(api.sliceCalls.every((c) => c.q.t === 9)).toBe(true)
    expect(api.volumeCalls.every((c) => c.q.t === 9)).toBe(true)
    expect(new Set(api.volumeCalls.map((c) => c.q.c))).toEqual(new Set([0, 1, 2]))
    s.dispose()
  })

  it('does not warm the view that is already active', async () => {
    vi.useFakeTimers()
    const api = new FakeApi()
    const s = session(api)
    s.update(navSnapshot(project, { activeView: 'xz', t: 3, generation: 1 }))
    await vi.advanceTimersByTimeAsync(200)
    expect(new Set(api.sliceCalls.map((c) => c.q.axis))).toEqual(new Set(['xz', 'yz']))
    s.dispose()
  })

  it('skips volume warming while the 3D view is the active one', async () => {
    vi.useFakeTimers()
    const api = new FakeApi()
    const s = session(api)
    s.update(navSnapshot(project, { activeView: '3d', t: 2, generation: 1 }))
    await vi.advanceTimersByTimeAsync(200)
    // the active 3D view fetched t=2 and prefetched t=3; the warmer added nothing else
    expect(new Set(api.volumeCalls.map((c) => c.q.t))).toEqual(new Set([2, 3]))
    s.dispose()
  })

  it('routes status from the active view and keeps its identity while it holds', async () => {
    const api = new FakeApi()
    const s = session(api)
    const nav = navSnapshot(project, { activeView: '3d' })
    s.update(nav)
    expect(s.status.awaitingFrame).toBe(true)
    await settle()
    expect(s.status.display.level).toBe(2)
    expect(s.status.awaitingFrame).toBe(false)
    const held = s.status
    s.update(nav)
    expect(s.status).toBe(held)
    s.dispose()
  })

  it('notifies once the active scene has data to draw', async () => {
    const api = new FakeApi()
    const s = session(api)
    let notified = 0
    s.onChange(() => (notified += 1))
    s.update(navSnapshot(project, { activeView: 'xz' }))
    await settle()
    expect(notified).toBeGreaterThan(0)
    expect(s.status.awaitingFrame).toBe(false)
    s.dispose()
  })

  it('drops cached pixels for a layer when its version bumps', async () => {
    const api = new FakeApi()
    const s = session(api)
    s.update(navSnapshot(project, { activeView: 'xz' }))
    await settle()
    const before = s.planes.stats.entries
    s.invalidate('image', 2)
    expect(before).toBeGreaterThan(0)
    expect(s.planes.stats.entries).toBe(0)
    s.dispose()
  })

  it('refetches overlays after a graph change', async () => {
    const api = new FakeApi()
    api.cells = [cell(1, 0, [1, 10, 10])]
    const s = session(api)
    s.update(navSnapshot(project, { activeView: 'xy' }))
    await settle()
    const calls = api.cellCalls.length
    s.graphChanged()
    s.update(navSnapshot(project, { activeView: 'xy', generation: 2 }))
    await settle()
    expect(api.cellCalls.length).toBe(calls + 1)
    s.dispose()
  })

  it('stays silent for a camera-only update while still reporting the new zoom', async () => {
    const api = new FakeApi()
    const s = session(api)
    const nav = navSnapshot(project, { activeView: '3d' })
    s.update(nav)
    await settle()
    let notified = 0
    s.onChange(() => (notified += 1))
    s.update(moved(nav, -3))
    // An emit here would bump SceneCanvas's tick and rebuild the volume layer every frame.
    expect(notified).toBe(0)
    expect(s.status.display.zoom).toBe(-3)
    s.dispose()
  })

  it('issues no request of any kind while the camera moves', async () => {
    const api = new HeldCellsApi()
    api.auto = false
    const s = session(api)
    const nav = navSnapshot(project, { activeView: '3d' })
    s.update(nav)
    await settle()
    expect(api.openVolumes).toBeGreaterThan(0)
    expect(api.openCells).toBe(1)
    const before = {
      volume: api.volumeCalls.length,
      slice: api.sliceCalls.length,
      cells: api.cellCalls.length,
      pixel: api.pixelCalls.length,
      aborted: api.volumeCalls.map((c) => c.signal?.aborted ?? false),
    }

    for (let i = 1; i <= 12; i += 1) s.update(moved(nav, -i / 4))
    await settle()

    expect(api.volumeCalls).toHaveLength(before.volume)
    expect(api.cellCalls).toHaveLength(before.cells)
    expect(api.sliceCalls).toHaveLength(before.slice)
    expect(api.pixelCalls).toHaveLength(before.pixel)
    expect(api.volumeCalls.map((c) => c.signal?.aborted ?? false)).toEqual(before.aborted)
    expect(s.status.display.zoom).toBe(-3)
    s.dispose()
  })

  it('shares one plane cache across the slice views', () => {
    const api = new FakeApi()
    const s = session(api)
    expect(s.scene('xz')).toBe(s.slices.xz)
    expect(s.scene('3d')).toBe(s.volumeScene)
    s.dispose()
  })

  it('resolves the cursor readout against its own track window', async () => {
    const api = new FakeApi()
    api.pixelValue = 300
    api.labelValue = 42
    api.cells = [cell(42, 0, [1, 10, 10], 9)]
    const s = session(api)
    s.update(navSnapshot(project, { activeView: 'xy' }))
    await settle()
    s.readout.move([1, 10, 10], { t: 0, channel: 0, labels: true })
    await settle()
    expect(s.readout.sample).toMatchObject({ value: 300, labelId: 42, trackId: 9 })
    s.dispose()
  })
})

describe('ViewerSession lifecycle', () => {
  const first = devProject()
  const second = devProject({
    sessionId: 'session-2',
    versions: { sessionId: 'session-2', image: 1, labels: 1, graph: 1, settings: 1 },
  })

  it('adopts the session of the project it is first driven with', async () => {
    const api = new FakeApi()
    const s = session(api)
    expect(s.sessionId).toBe(null)
    s.update(navSnapshot(first, { activeView: 'xz' }))
    await settle()
    expect(s.sessionId).toBe('session-1')
    s.dispose()
  })

  it('tears the scenes and caches down when the session changes', async () => {
    const api = new FakeApi()
    api.cells = [cell(1, 0, [1, 10, 10])]
    const s = session(api)
    s.update(navSnapshot(first, { activeView: 'xz' }))
    await settle()
    s.readout.move([1, 10, 10], { t: 0, channel: 0 })
    await settle()
    expect(s.planes.stats.entries).toBeGreaterThan(0)
    expect(s.slices.xz.plane).not.toBe(null)
    expect(s.tracks.cells).toHaveLength(1)

    s.update(navSnapshot(second, { activeView: 'xz', generation: 2 }))
    expect(s.sessionId).toBe('session-2')
    expect(s.planes.stats.entries).toBe(0)
    expect(s.volumes.stats.entries).toBe(0)
    expect(s.slices.xy.plane).toBe(null)
    expect(s.volumeScene.volume).toBe(null)
    expect(s.tracks.cells).toHaveLength(0)
    expect(s.readout.sample).toBe(null)
    s.dispose()
  })

  it('never serves a plane cached under the previous session', async () => {
    const api = new FakeApi()
    const s = session(api)
    const nav = (sessionId: string, generation: number) =>
      navSnapshot(sessionId === 'session-1' ? first : second, {
        activeView: 'xz',
        index: { xz: 512 },
        generation,
      })
    s.update(nav('session-1', 1))
    await settle()
    const fetched = api.sliceCalls.filter((c) => c.q.pos === 512).length
    expect(fetched).toBe(1)

    // Same key in every field the cache hashes; only the session behind it differs.
    s.update(nav('session-2', 2))
    await settle()
    expect(api.sliceCalls.filter((c) => c.q.pos === 512)).toHaveLength(fetched + 1)
    s.dispose()
  })

  it('leaves nothing running after dispose', async () => {
    vi.useFakeTimers()
    const api = new FakeApi()
    const s = session(api, 10)
    let notified = 0
    s.onChange(() => (notified += 1))
    s.update(navSnapshot(first, { activeView: 'xz' }))
    await vi.advanceTimersByTimeAsync(20)
    expect(notified).toBeGreaterThan(0)

    s.dispose()
    const after = notified
    const calls = api.sliceCalls.length
    s.slices.xz.setCamera({ target: [1, 1], zoom: -1 })
    await vi.advanceTimersByTimeAsync(200)
    expect(notified).toBe(after)
    expect(api.sliceCalls).toHaveLength(calls)
    expect(s.planes.stats.entries).toBe(0)
    expect(s.status).toEqual({ display: { level: 0, zoom: 0 }, awaitingFrame: false })
    vi.useRealTimers()
  })
})

describe('ViewerSession label version path', () => {
  const project = devProject({ hasLabels: true })
  const labelKey = (version: number, index = 512) => ({
    layer: 'labels' as const,
    axis: 'xz' as const,
    level: 0,
    t: 0,
    c: [0],
    index,
    version,
  })
  const imageKey = {
    layer: 'image' as const,
    axis: 'xz' as const,
    level: 0,
    t: 0,
    c: [0, 1, 2],
    index: 512,
    version: 1,
  }
  const opened = async (api: FakeApi) => {
    const s = session(api)
    s.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 } }))
    await settle()
    return s
  }
  const fetched = (api: FakeApi, version: number) =>
    api.sliceCalls.filter((c) => c.q.layer === 'labels' && c.q.pos === 512).length === version

  it('fetches once whichever of the response and the event lands first', async () => {
    const api = new FakeApi()
    const s = await opened(api)
    const at512 = () => api.sliceCalls.filter((c) => c.q.layer === 'labels' && c.q.pos === 512)
    expect(at512()).toHaveLength(1)

    expect(s.advanceLabels('session-1', 2)).toBe(true)
    await settle()
    expect(at512()).toHaveLength(2)
    // The matching event arrives after the response, and does nothing.
    expect(s.advanceLabels('session-1', 2)).toBe(false)
    await settle()
    expect(at512()).toHaveLength(2)
    expect(fetched(api, 2)).toBe(true)
    s.dispose()
  })

  it('ignores a version from another backend session', async () => {
    const api = new FakeApi()
    const s = await opened(api)
    const before = api.sliceCalls.length
    expect(s.advanceLabels('other-session', 9)).toBe(false)
    await settle()
    expect(api.sliceCalls).toHaveLength(before)
    s.dispose()
  })

  it('drops label entries and leaves the image resident', async () => {
    const api = new FakeApi()
    const s = await opened(api)
    expect(s.planes.has(labelKey(1))).toBe(true)
    s.advanceLabels('session-1', 2)
    await settle()
    expect(s.planes.has(labelKey(1))).toBe(false)
    expect(s.planes.has(labelKey(2))).toBe(true)
    expect(s.planes.has(imageKey)).toBe(true)
    s.dispose()
  })

  it("patches the response's cells into the overlay without a /cells fetch", async () => {
    const api = new FakeApi()
    const s = session(api)
    // A frame whose trail window is already loaded, so no `ensure` re-reads over the patch.
    s.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, t: 20 }))
    await settle()
    const before = api.cellCalls.length
    s.advanceLabels('session-1', 3, [cell(77, 20, [1, 10, 20], 5)], [])
    await settle()
    expect(s.tracks.cell(77)?.trackId).toBe(5)
    expect(api.cellCalls).toHaveLength(before)
    s.advanceLabels('session-1', 4, [], [77])
    expect(s.tracks.cell(77)).toBe(null)
    s.dispose()
  })

  it('reserves a block of ids when a paint tool becomes active, and not before', async () => {
    const api = new FakeApi()
    const s = session(api)
    s.update(navSnapshot(project, { activeView: 'xy' }))
    await settle()
    expect(api.reserveCalls).toHaveLength(0)
    s.update(navSnapshot(project, { activeView: 'xy', tool: 'brush' }))
    await settle()
    expect(api.reserveCalls).toEqual([64])
    s.dispose()
  })
})
