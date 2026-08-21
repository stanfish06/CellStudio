import { describe, expect, it } from 'vitest'
import { VolumeScene } from './volumeScene'
import { GpuBudget } from '../data/gpuBudget'
import { VolumeCache } from '../data/volumeCache'
import { TrackSource } from '../data/trackSource'
import { PerfMonitor } from '../perf'
import { FakeApi, cell, devProject, layerProps, makeVolume, navSnapshot } from '../test/data'
import type { PickingInfo } from '@deck.gl/core'

const settle = () => new Promise((r) => setTimeout(r, 0))

const setup = (auto = true) => {
  const api = new FakeApi()
  api.auto = auto
  const volumes = new VolumeCache({ api, maxConcurrent: 8 })
  const budget = new GpuBudget({
    totalBytes: 512 * 1024 * 1024,
    volumeCeilingBytes: 8 * 1024 * 1024,
  })
  return { api, volumes, budget }
}

describe('VolumeScene', () => {
  const project = devProject()

  it('fetches one volume per visible channel at the budgeted level', async () => {
    const { api, volumes, budget } = setup()
    const scene = new VolumeScene({ volumes, budget })
    scene.update(navSnapshot(project, { activeView: '3d', t: 5 }))
    await settle()
    expect(scene.volumeLevel).toBe(2)
    expect(api.volumeCalls.filter((c) => c.q.t === 5).map((c) => c.q.c)).toEqual([0, 1, 2])
    expect(api.volumeCalls.every((c) => c.q.level === 2)).toBe(true)
    expect(scene.volume?.channels).toHaveLength(3)
  })

  it('prefetches the next timepoint, so the step onto it costs no fetch', async () => {
    const { api, volumes, budget } = setup()
    const scene = new VolumeScene({ volumes, budget })
    scene.update(navSnapshot(project, { activeView: '3d', t: 5, generation: 1 }))
    await settle()
    expect(new Set(api.volumeCalls.map((c) => c.q.t))).toEqual(new Set([5, 6]))
    api.volumeCalls.length = 0
    scene.update(navSnapshot(project, { activeView: '3d', t: 6, generation: 2 }))
    await settle()
    expect(new Set(api.volumeCalls.map((c) => c.q.t))).toEqual(new Set([7]))
    expect(scene.volume?.t).toBe(6)
  })

  it('never lets a late volume replace a newer timepoint', async () => {
    const { api, volumes, budget } = setup(false)
    const scene = new VolumeScene({ volumes, budget })
    scene.update(navSnapshot(project, { activeView: '3d', t: 5, generation: 1 }))
    scene.update(navSnapshot(project, { activeView: '3d', t: 6, generation: 2 }))
    const at = (t: number, c: number) =>
      api.volumeCalls.findIndex((call) => call.q.t === t && call.q.c === c)
    expect(api.volumeCalls[at(5, 0)]?.signal?.aborted).toBe(true)
    for (const c of [0, 1, 2]) api.settleVolume(at(6, c), makeVolume(3, 256, 256))
    for (const c of [0, 1, 2]) api.settleVolume(at(5, c), makeVolume(3, 128, 128))
    await settle()
    expect(scene.volume?.t).toBe(6)
    expect(scene.status().awaitingFrame).toBe(false)
  })

  it('scales the volume box by voxel size, level factor and display scale', async () => {
    const { volumes, budget } = setup()
    const scene = new VolumeScene({ volumes, budget })
    scene.update(navSnapshot(project, { activeView: '3d', axisScale: { z: 4, y: 1, x: 1 } }))
    await settle()
    const scaling = layerProps(scene.layers()[0]).physicalSizeScalingMatrix as {
      transformPoint(p: number[]): number[]
    }
    // level 2 is 4x downsampled in xy, so one texel spans 4 pixels of x.
    expect(scaling.transformPoint([256, 256, 3])).toEqual([1024, 1024, 3 * (2.0 / 0.603) * 4])
    expect(scene.extent()[2]).toBeCloseTo(3 * (2.0 / 0.603) * 4, 6)
  })

  it('orbits with no fetches — camera changes alone do not request data', async () => {
    const { api, volumes, budget } = setup()
    const scene = new VolumeScene({ volumes, budget })
    const nav = navSnapshot(project, { activeView: '3d', t: 2 })
    scene.update(nav)
    await settle()
    const before = api.volumeCalls.length
    scene.setViewport({ width: 900, height: 700 })
    const orbited = {
      ...nav,
      volume: { camera: { rotationX: 40, rotationOrbit: 90, zoom: -1, target: [10, 10, 3] } },
    }
    scene.update(orbited as typeof nav)
    await settle()
    expect(api.volumeCalls).toHaveLength(before)
    expect(scene.viewState().rotationOrbit).toBe(90)
  })

  it('uses an orbit view with viv Y axis convention and a fit camera', () => {
    const { volumes, budget } = setup()
    const scene = new VolumeScene({ volumes, budget })
    scene.setViewport({ width: 800, height: 800 })
    scene.update(navSnapshot(project, { activeView: '3d' }))
    expect(scene.view().props.orbitAxis).toBe('Y')
    const state = scene.viewState()
    expect(Number.isFinite(state.zoom)).toBe(true)
    expect(state.target[2]).toBeCloseTo((3 * (2.0 / 0.603)) / 2, 6)
  })

  it('selects on a centroid pick and jumps on double-click', async () => {
    const api = new FakeApi()
    api.cells = [cell(11, 3, [1, 100, 200], 7)]
    const volumes = new VolumeCache({ api })
    const tracks = new TrackSource(api)
    const selected: number[] = []
    const jumped: number[] = []
    const scene = new VolumeScene({
      volumes,
      tracks,
      onSelect: (c) => selected.push(c.id),
      onJumpToCell: (c) => jumped.push(c.id),
    })
    scene.update(navSnapshot(project, { activeView: '3d', t: 3 }))
    await settle()
    const info = { object: { cellId: 11 } } as unknown as PickingInfo
    expect(scene.handlePick(info)?.id).toBe(11)
    expect(scene.handlePick(info, true)?.id).toBe(11)
    expect(selected).toEqual([11])
    expect(jumped).toEqual([11])
    expect(scene.handlePick({ object: undefined } as PickingInfo)).toBe(null)
  })

  it('closes the 3D time-step interaction at the presented frame', async () => {
    const { volumes, budget } = setup()
    const perf = new PerfMonitor()
    const scene = new VolumeScene({ volumes, budget, perf })
    scene.update(navSnapshot(project, { activeView: '3d', t: 1 }))
    await settle()
    scene.markPresented()
    expect(perf.stats('t-step-3d').n).toBe(1)
  })

  it('carries the rendering mode into the layer extension', () => {
    const { volumes, budget } = setup()
    const scene = new VolumeScene({ volumes, budget, renderingMode: 'mip' })
    expect(scene.mode).toBe('mip')
    scene.setRenderingMode('additive')
    expect(scene.mode).toBe('additive')
  })

  it('reports the resident level and the live orbit zoom', async () => {
    const { api, volumes, budget } = setup()
    const scene = new VolumeScene({ volumes, budget })
    scene.update(navSnapshot(project, { activeView: '3d', t: 1 }))
    await settle()
    const before = api.volumeCalls.length
    scene.setCamera({ rotationX: 40, rotationOrbit: 90, zoom: -1.5, target: [1, 2, 3] })
    expect(scene.status()).toEqual({ display: { level: 2, zoom: -1.5 }, awaitingFrame: false })
    expect(scene.viewState().rotationOrbit).toBe(90)
    expect(api.volumeCalls).toHaveLength(before)
  })
})
