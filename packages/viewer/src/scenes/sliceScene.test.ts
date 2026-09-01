import { describe, expect, it, vi } from 'vitest'
import type { PickingInfo } from '@deck.gl/core'
import type { PlaneBuffer } from '@cellstudio/api-client'
import { SliceScene, lineageHighlight } from './sliceScene'
import { PlaneCache } from '../data/planeCache'
import { RemapCache } from '../data/trackFrame'
import { TrackSource } from '../data/trackSource'
import { MaskEditor } from '../edit/maskEditor'
import { labelColor } from '../layers/labelPalette'
import { trackColor } from '../layers/tracks'
import { PerfMonitor } from '../perf'
import {
  FakeApi,
  cell,
  devProject,
  layerProps,
  makePlane,
  navSnapshot,
  type SliceCall,
} from '../test/data'

const setup = (auto = true) => {
  const api = new FakeApi()
  api.auto = auto
  const planes = new PlaneCache({ api })
  const perf = new PerfMonitor()
  return { api, planes, perf }
}

const labelEditor = (api: FakeApi, storePresent: boolean): MaskEditor => {
  const editor = new MaskEditor({ api, onCommit: () => {} })
  editor.configure({ dims: [3, 1024, 1024], scale: null, storePresent })
  return editor
}

const labelCalls = (api: FakeApi) => api.sliceCalls.filter((c) => c.q.layer === 'labels')

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

describe('SliceScene label overlay', () => {
  const project = devProject({ hasLabels: true })

  it('requests nothing while the project has no label store', async () => {
    const { api, planes } = setup()
    const scene = new SliceScene({
      orientation: 'xz',
      planes,
      api,
      editor: labelEditor(api, false),
    })
    scene.update(navSnapshot(devProject(), { activeView: 'xz' }))
    await settle()
    expect(labelCalls(api)).toHaveLength(0)
    expect(scene.layers().some((l) => l.id.endsWith('-labels'))).toBe(false)
  })

  it('requests the label plane for the current slice and draws it over the image', async () => {
    const { api, planes } = setup()
    const scene = new SliceScene({
      orientation: 'xz',
      planes,
      api,
      editor: labelEditor(api, true),
    })
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 } }))
    await settle()
    expect(labelCalls(api)[0]?.q).toMatchObject({ axis: 'xz', pos: 512, cs: [0], level: 0, t: 0 })
    const ids = scene.layers().map((l) => l.id)
    expect(ids.indexOf('slice-xz-labels')).toBe(ids.indexOf('slice-xz-plane') + 1)
    expect(layerProps(scene.layers()[1]).labelOpacity).toBeCloseTo(0.36, 6)
  })

  it("re-requests on a zoom that moves the level, in that level's coordinates", async () => {
    const { api, planes } = setup()
    const scene = new SliceScene({
      orientation: 'xz',
      planes,
      api,
      editor: labelEditor(api, true),
    })
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 } }))
    await settle()
    api.sliceCalls.length = 0
    scene.setCamera({ target: [512, 5], zoom: -2 })
    await settle()
    // level 2 is 4x downsampled in y, and `/slice` indexes in level coordinates.
    expect(labelCalls(api)[0]?.q).toMatchObject({ level: 2, pos: 128 })
    expect(api.sliceCalls.some((c) => c.q.layer === 'image')).toBe(false)
  })

  it('selects the cell under a label voxel, through its own pixel lookup', async () => {
    const { api, planes } = setup()
    api.labelValue = 42
    const selected: number[] = []
    const scene = new SliceScene({
      orientation: 'xy',
      planes,
      api,
      editor: labelEditor(api, true),
      onSelectLabel: (id) => selected.push(id),
    })
    scene.update(navSnapshot(project, { index: { xy: 1 } }))
    scene.handlePick({ coordinate: [100, 200] } as unknown as PickingInfo)
    await settle()
    expect(selected).toEqual([42])
    expect(api.pixelCalls[0]).toMatchObject({ layer: 'labels', t: 0, c: 0, z: 1, y: 200, x: 100 })
  })

  it('draws the brush cursor and resizes it with no request', async () => {
    const { api, planes } = setup()
    const scene = new SliceScene({
      orientation: 'xy',
      planes,
      api,
      editor: labelEditor(api, true),
    })
    scene.update(navSnapshot(project, { tool: 'brush', brushRadius: 8 }))
    await settle()
    scene.setPointer([100, 200])
    const before = api.sliceCalls.length
    const cursor = () => scene.layers().find((l) => l.id === 'slice-xy-brush-cursor')
    expect(layerProps(cursor()).getRadius).toBe(8)
    scene.update(navSnapshot(project, { tool: 'brush', brushRadius: 20 }))
    await settle()
    expect(layerProps(cursor()).getRadius).toBe(20)
    expect(api.sliceCalls).toHaveLength(before)
  })

  it('draws no cursor without a paint tool', () => {
    const { api, planes } = setup()
    const scene = new SliceScene({ orientation: 'xy', planes, api })
    scene.update(navSnapshot(project, { tool: 'pointer' }))
    scene.setPointer([100, 200])
    expect(scene.layers().some((l) => l.id.endsWith('-brush-cursor'))).toBe(false)
  })
})

/** Label planes filled with one id per frame, so the remap output is observable. */
class LabelValueApi extends FakeApi {
  labelIdFor: (t: number) => number = () => 0

  override slice(q: SliceCall['q'], signal?: AbortSignal): Promise<PlaneBuffer> {
    return super.slice(q, signal).then((plane) => {
      if (q.layer === 'labels') new Uint32Array(plane.data).fill(this.labelIdFor(q.t))
      return plane
    })
  }
}

describe('SliceScene track-colored labels.', () => {
  const project = devProject({ hasLabels: true, hasGraph: true })

  const setupTracked = () => {
    const api = new LabelValueApi()
    api.labelIdFor = (t) => (t === 0 ? 77 : 78)
    api.cells = [cell(77, 0, [512, 100, 200], 5), cell(78, 1, [512, 105, 205], 5)]
    const planes = new PlaneCache({ api })
    const tracks = new TrackSource(api)
    const remaps = new RemapCache()
    const scene = new SliceScene({
      orientation: 'xz',
      planes,
      tracks,
      remaps,
      api,
      editor: labelEditor(api, true),
    })
    return { api, scene, tracks, remaps }
  }

  const labelLayer = (scene: SliceScene) =>
    layerProps(scene.layers().find((l) => l.id.endsWith('-labels')))
  const shownId = (scene: SliceScene): number => {
    const channelData = labelLayer(scene).channelData as { data: Uint32Array[] }
    return channelData.data[0]?.[0] ?? -1
  }

  it('renders the mask fill in the trail color for one tracked cell across two frames', async () => {
    const { scene } = setupTracked()
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, t: 0 }))
    await settle()
    // voxel 77 displays as track 5 — the exact value the trail keys its color on
    expect(shownId(scene)).toBe(5)
    scene.update(
      navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, t: 1, generation: 2 }),
    )
    await settle()
    expect(shownId(scene)).toBe(5)
    expect(trackColor(5)).toEqual(labelColor(5))
  })

  it('keeps masks track-colored when the track overlay is hidden', async () => {
    const { api, scene } = setupTracked()
    scene.update(
      navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, tracks: { on: false } }),
    )
    await settle()
    expect(api.cellCalls).toHaveLength(1)
    expect(shownId(scene)).toBe(5)
    expect(scene.layers().some((l) => l.id.endsWith('-tracks'))).toBe(false)
  })

  it('translates the selected cell id to its track id for the shader highlight', async () => {
    const { scene } = setupTracked()
    scene.update(
      navSnapshot(project, {
        activeView: 'xz',
        index: { xz: 512 },
        selection: { cellId: 77 },
      }),
    )
    await settle()
    expect(labelLayer(scene).selectedLabel).toBe(5)
  })

  it('renders the canonical buffer until /cells lands, then swaps in the remap', async () => {
    const api = new LabelValueApi()
    api.labelIdFor = () => 77
    api.cells = [cell(77, 0, [512, 100, 200], 5)]
    const held: ((rows: typeof api.cells) => void)[] = []
    api.cellsWindow = (q) => {
      api.cellCalls.push({ t0: q.t0, t1: q.t1 })
      return new Promise((resolve) => held.push(resolve))
    }
    const planes = new PlaneCache({ api })
    const tracks = new TrackSource(api)
    const remaps = new RemapCache()
    const scene = new SliceScene({
      orientation: 'xz',
      planes,
      tracks,
      remaps,
      api,
      editor: labelEditor(api, true),
    })
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 } }))
    await settle()
    // the label plane beat /cells: canonical ids, nothing cached under this graph version
    expect(shownId(scene)).toBe(77)
    expect(remaps.stats.entries).toBe(0)
    held[0]?.(api.cells)
    await settle()
    expect(shownId(scene)).toBe(5)
    expect(remaps.stats.entries).toBe(1)
  })

  it('renders identical colors to the label-id scheme when no tracks are loaded', async () => {
    const { api, scene, remaps } = setupTracked()
    api.cells = []
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 } }))
    await settle()
    // canonical buffer, untouched: same values, same shader palette, same colors
    expect(shownId(scene)).toBe(77)
    expect(remaps.stats.entries).toBe(0)
    expect(labelLayer(scene).selectedLabel).toBe(0)
  })

  it('makes zero /cells calls for opacity and fade changes (task 5.2)', async () => {
    const { api, scene } = setupTracked()
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 } }))
    await settle()
    expect(api.cellCalls).toHaveLength(1)
    scene.update(
      navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, tracks: { opacity: 0.4 } }),
    )
    scene.update(
      navSnapshot(project, {
        activeView: 'xz',
        index: { xz: 512 },
        tracks: { fade: { on: false, max: 0.7, min: 0.1 } },
      }),
    )
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, trail: 3 }))
    await settle()
    expect(api.cellCalls).toHaveLength(1)
  })

  it('passes the fade bounds through to the track layer', async () => {
    const { scene } = setupTracked()
    const fade = { on: true, max: 0.8, min: 0.2 }
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, tracks: { fade } }))
    await settle()
    const overlay = layerProps(scene.layers().find((l) => l.id.endsWith('-tracks')))
    expect(overlay.fade).toEqual(fade)
  })

  it("highlights only the selected cell's own history, not cousin branches", () => {
    // 1 divides into 2 and 3; 2 continues to 4. Selecting 4 must not light up 3.
    const lineage = {
      graphVersion: 1,
      focusCellId: 4,
      cells: [
        cell(1, 0, [1, 1, 1]),
        cell(2, 1, [1, 2, 2]),
        cell(3, 1, [1, 3, 3]),
        cell(4, 2, [1, 4, 4]),
      ],
      links: [
        { parent: 1, child: 2 },
        { parent: 1, child: 3 },
        { parent: 2, child: 4 },
      ],
    }
    const { set, overlay } = lineageHighlight(lineage, 4)
    expect([...(set ?? [])].sort((a, b) => a - b)).toEqual([1, 2, 4])
    expect(overlay?.links).toEqual([
      { parent: 2, child: 4 },
      { parent: 1, child: 2 },
    ])
    expect(overlay?.cells.map((c) => c.id)).toEqual([1, 2, 4])
  })

  it('falls back to the selected cell alone until its lineage lands', () => {
    expect(lineageHighlight(null, 7).set).toEqual(new Set([7]))
    expect(lineageHighlight(null, null).set).toBeUndefined()
  })

  it('a trail pick selects that edge and never a cell', async () => {
    const api = new LabelValueApi()
    api.cells = [cell(77, 0, [512, 100, 200], 5), cell(78, 1, [512, 105, 205], 5)]
    const picked: { parent: number; child: number }[] = []
    const cells: number[] = []
    const scene = new SliceScene({
      orientation: 'xz',
      planes: new PlaneCache({ api }),
      tracks: new TrackSource(api),
      remaps: new RemapCache(),
      api,
      onSelect: (c) => cells.push(c.id),
      onSelectLink: (link) => picked.push(link),
    })
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 } }))
    await settle()

    const segment = { fromCellId: 77, toCellId: 78 } as unknown as PickingInfo['object']
    expect(scene.handlePick({ object: segment } as PickingInfo)).toBe(null)
    expect(picked).toEqual([{ parent: 77, child: 78 }])
    expect(cells).toEqual([])
  })

  it('passes the selected edge through to the track layer', async () => {
    const { scene } = setupTracked()
    const selectedLink = { parent: 77, child: 78 }
    scene.update(navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, selectedLink }))
    await settle()
    const overlay = layerProps(scene.layers().find((l) => l.id.endsWith('-tracks')))
    expect(overlay.selectedLink).toEqual(selectedLink)
  })

  it('passes the dot size through to the track layer', async () => {
    const { scene } = setupTracked()
    scene.update(
      navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, tracks: { dotSize: 9 } }),
    )
    await settle()
    const overlay = layerProps(scene.layers().find((l) => l.id.endsWith('-tracks')))
    expect(overlay.dotSize).toBe(9)
  })

  it('holds the overlay on the shown frame until the image for the new one lands', async () => {
    const api = new LabelValueApi()
    api.auto = false
    api.cells = [cell(77, 0, [512, 100, 200], 5), cell(78, 1, [512, 105, 205], 5)]
    const scene = new SliceScene({
      orientation: 'xz',
      planes: new PlaneCache({ api }),
      tracks: new TrackSource(api),
      remaps: new RemapCache(),
      api,
    })
    const at = (t: number) => navSnapshot(project, { activeView: 'xz', index: { xz: 512 }, t })
    const trackT = () =>
      layerProps(scene.layers().find((l) => l.id.endsWith('-tracks'))).t as number
    // held responses stay in the array once settled, so resolve every index
    const deliver = async () => {
      for (let i = 0; i < api.openSlices; i += 1) api.settleSlice(i)
      await settle()
    }

    scene.update(at(0))
    await settle()
    await deliver()
    expect(trackT()).toBe(0)

    // t moved, the image for it is still in flight: overlays stay on frame 0
    scene.update(at(1))
    await settle()
    expect(trackT()).toBe(0)

    await deliver()
    expect(trackT()).toBe(1)
  })
})
