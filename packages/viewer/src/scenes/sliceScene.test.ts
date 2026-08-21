import { describe, expect, it, vi } from 'vitest'
import type { PickingInfo } from '@deck.gl/core'
import { SliceScene } from './sliceScene'
import { PlaneCache } from '../data/planeCache'
import { TrackSource } from '../data/trackSource'
import { PerfMonitor } from '../perf'
import { FakeApi, cell, devProject, layerProps, makePlane, navSnapshot } from '../test/data'

const setup = (auto = true) => {
  const api = new FakeApi()
  api.auto = auto
  const planes = new PlaneCache({ api })
  const perf = new PerfMonitor()
  return { api, planes, perf }
}

const settle = () => new Promise((r) => setTimeout(r, 0))

describe('SliceScene ortho path', () => {
  const project = devProject()

  it('requests the plane for the current nav key and prefetches ahead', async () => {
    const { api, planes } = setup()
    const scene = new SliceScene({ orientation: 'xz', planes })
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 } }))
    await settle()
    expect(api.sliceCalls[0]?.q).toMatchObject({ axis: 'xz', pos: 512, t: 0, cs: [0, 1, 2] })
    expect(scene.plane?.shape).toEqual([3, 1024])
    // adjacent plane plus the next y-brick
    expect(api.sliceCalls.slice(1).map((c) => c.q.pos)).toEqual([513, 768])
  })

  it('prefetches backwards once the scrub direction reverses', async () => {
    const { api, planes } = setup()
    const scene = new SliceScene({ orientation: 'xz', planes })
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, generation: 1 }))
    await settle()
    api.sliceCalls.length = 0
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 511 }, generation: 2 }))
    await settle()
    expect(scene.scrubDirection).toBe(-1)
    expect(api.sliceCalls.map((c) => c.q.pos)).toEqual([511, 510, 255])
  })

  it('never lets a late plane replace a newer one', async () => {
    const { api, planes } = setup(false)
    const scene = new SliceScene({ orientation: 'xz', planes })
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 100 }, generation: 1 }))
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 110 }, generation: 2 }))
    const at = (pos: number) => api.sliceCalls.findIndex((c) => c.q.pos === pos)
    expect(api.sliceSignal(at(100))?.aborted).toBe(true)
    api.settleSlice(at(110), makePlane(3, 1024, 3, 0))
    api.settleSlice(at(100), makePlane(3, 512, 3, 1))
    await settle()
    expect(scene.plane?.shape).toEqual([3, 1024])
    expect(scene.status().awaitingFrame).toBe(false)
  })

  it('reports an outstanding fetch so playback can skip a tick', () => {
    const { planes } = setup(false)
    const scene = new SliceScene({ orientation: 'xz', planes })
    expect(scene.status().awaitingFrame).toBe(false)
    scene.update(navSnapshot(project, { activeView: 'xz' }))
    expect(scene.status().awaitingFrame).toBe(true)
  })

  it('does not refetch when nav has not moved', async () => {
    const { api, planes } = setup()
    const scene = new SliceScene({ orientation: 'xz', planes })
    const nav = navSnapshot(project, { activeView: 'xz' })
    scene.update(nav)
    await settle()
    const calls = api.sliceCalls.length
    scene.update(nav)
    await settle()
    expect(api.sliceCalls).toHaveLength(calls)
  })

  it('closes the interaction at the presented frame', async () => {
    const { planes, perf } = setup()
    const scene = new SliceScene({ orientation: 'xz', planes, perf })
    scene.update(navSnapshot(project, { activeView: 'xz' }))
    await settle()
    scene.markPresented()
    expect(perf.stats('ortho-step').n).toBe(1)
    expect(perf.readout().frames).toBe(0)
  })

  it('sizes the quad in world units so anisotropy and display scale apply', async () => {
    const { planes } = setup()
    const scene = new SliceScene({ orientation: 'xz', planes })
    scene.setViewport({ width: 1200, height: 800 })
    scene.update(navSnapshot(project, { activeView: 'xz', axisScale: { z: 8, y: 1, x: 1 } }))
    await settle()
    const bounds = layerProps(scene.layers()[0]).bounds as number[]
    expect(bounds[2]).toBe(1024)
    expect(bounds[1]).toBeCloseTo(3 * (2.0 / 0.603) * 8, 6)
    expect(Number.isFinite(scene.fit().zoom)).toBe(true)
  })

  it('renders no image layer until a plane commits', () => {
    const { planes } = setup(false)
    const scene = new SliceScene({ orientation: 'yz', planes })
    scene.update(navSnapshot(project, { activeView: 'yz' }))
    expect(scene.layers()).toEqual([])
  })
})

describe('SliceScene display state', () => {
  const project = devProject()

  it('reports the level and zoom the live camera puts on screen', async () => {
    const { api, planes } = setup()
    const scene = new SliceScene({ orientation: 'xz', planes })
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 } }))
    await settle()
    expect(scene.status().display.level).toBe(0)

    api.sliceCalls.length = 0
    scene.setCamera({ target: [512, 5], zoom: -2 })
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, generation: 2 }))
    await settle()
    expect(api.sliceCalls[0]?.q.level).toBe(2)
    expect(scene.status()).toEqual({ display: { level: 2, zoom: -2 }, awaitingFrame: false })
  })

  it('lets a nav jump outrank a camera the user left behind', async () => {
    const { planes } = setup()
    const scene = new SliceScene({ orientation: 'xy', planes })
    scene.setViewport({ width: 1024, height: 1024 })
    scene.update(navSnapshot(project))
    scene.setCamera({ target: [700, 700], zoom: -2 })
    expect(scene.viewState().target).toEqual([700, 700, 0])

    const jumped = navSnapshot(project)
    jumped.slices.xy.camera = { target: [10, 20], zoom: 1 }
    scene.update(jumped)
    expect(scene.viewState()).toEqual({ target: [10, 20, 0], zoom: 1 })
  })

  it('keeps the live camera across nav changes that do not move it', () => {
    const { planes } = setup()
    const scene = new SliceScene({ orientation: 'xy', planes })
    scene.update(navSnapshot(project, { t: 0 }))
    scene.setCamera({ target: [700, 700], zoom: -2 })
    // a fresh snapshot object, same camera values — a t-step, not a jump
    scene.update(navSnapshot(project, { t: 1, generation: 2 }))
    expect(scene.viewState().zoom).toBe(-2)
  })
})

describe('SliceScene XY path', () => {
  const project = devProject()

  it('issues one warming read per z-brick, not per plane', async () => {
    const { planes } = setup()
    const scene = new SliceScene({ orientation: 'xy', planes, brick: { z: 2, y: 256, x: 256 } })
    const getTile = vi.fn().mockResolvedValue({ data: new Uint16Array(4), width: 2, height: 2 })
    scene.setPyramid({
      levels: [{ getTile }] as never,
      tileSize: 1024,
      dtype: 'Uint16',
      labels: ['t', 'c', 'z', 'y', 'x'],
      width: 1024,
      height: 1024,
    })
    scene.update(navSnapshot(project, { index: { xy: 0 }, generation: 1 }))
    scene.update(navSnapshot(project, { index: { xy: 1 }, generation: 2 }))
    await settle()
    // z brick of 2 on a 3-plane stack: both steps target the same next brick, plane 2.
    expect(getTile.mock.calls.map((c) => c[0].selection.z)).toEqual([2, 2, 2, 2, 2, 2])
    expect(new Set(getTile.mock.calls.map((c) => c[0].selection.c))).toEqual(new Set([0, 1, 2]))
    expect(planes.stats.started).toBe(0)
  })

  it('draws XY in pixel space, so stretching Z leaves it alone', () => {
    const { planes } = setup()
    const scene = new SliceScene({ orientation: 'xy', planes })
    scene.update(navSnapshot(project, { axisScale: { z: 8, y: 1, x: 1 } }))
    expect(scene.extent()).toMatchObject({ width: 1024, height: 1024 })
  })

  it('passes window, color and gamma to the multiscale layer without a refetch', () => {
    const { planes } = setup()
    const scene = new SliceScene({ orientation: 'xy', planes })
    scene.setPyramid({
      levels: [{ getTile: vi.fn(), tileSize: 1024, dtype: 'Uint16' }] as never,
      tileSize: 1024,
      dtype: 'Uint16',
      labels: ['t', 'c', 'z', 'y', 'x'],
      width: 1024,
      height: 1024,
    })
    const channels = navSnapshot(project).channels.map((c, i) => ({
      ...c,
      visible: i < 2,
      gamma: 0.5,
    }))
    scene.update(navSnapshot(project, { channels, t: 7, index: { xy: 1 } }))
    const layer = layerProps(scene.layers()[0])
    expect(layer.selections).toEqual([
      { t: 7, c: 0, z: 1 },
      { t: 7, c: 1, z: 1 },
    ])
    expect(layer.gammas).toEqual([0.5, 0.5])
    expect(layer.contrastLimits).toEqual([channels[0]?.window, channels[1]?.window])
    expect(layer.maxCacheSize).toBeGreaterThan(0)
  })
})

describe('SliceScene track overlay', () => {
  const project = devProject()

  it('selects a centroid-only detection on a pick, and jumps on double-click', async () => {
    const api = new FakeApi()
    api.cells = [cell(11, 0, [1, 100, 200], 7)]
    const planes = new PlaneCache({ api })
    const tracks = new TrackSource(api)
    const selected: number[] = []
    const jumped: number[] = []
    const scene = new SliceScene({
      orientation: 'xy',
      planes,
      tracks,
      onSelect: (c) => selected.push(c.id),
      onJumpToCell: (c) => jumped.push(c.id),
    })
    scene.update(navSnapshot(project, { index: { xy: 1 } }))
    await settle()
    const info = { object: { cellId: 11 } } as unknown as PickingInfo
    expect(scene.handlePick(info)?.trackId).toBe(7)
    expect(scene.handlePick(info, true)?.id).toBe(11)
    expect(selected).toEqual([11])
    expect(jumped).toEqual([11])
    expect(scene.handlePick({ object: undefined } as PickingInfo)).toBe(null)
  })

  it('slab-filters the overlay around the current plane', async () => {
    const api = new FakeApi()
    api.cells = [cell(1, 0, [1, 100, 200]), cell(2, 0, [2, 300, 400])]
    const planes = new PlaneCache({ api })
    const tracks = new TrackSource(api)
    const scene = new SliceScene({ orientation: 'xy', planes, tracks, slabRadius: 0 })
    scene.update(navSnapshot(project, { index: { xy: 1 } }))
    await settle()
    const overlay = layerProps(scene.layers().find((l) => l.id.endsWith('-tracks')))
    expect(overlay.index).toBe(1)
    expect(overlay.slab).toBe(0)
    expect(overlay.cells).toHaveLength(2)
    expect(api.cellCalls[0]).toMatchObject({ t0: 0 })
  })
})
