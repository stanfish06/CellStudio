import { create } from 'zustand'
import type { CellRow, ChannelMeta, ProjectInfo } from '@cellstudio/api-client'

export type ActiveView = 'xy' | 'xz' | 'yz' | '3d'
export type SliceOrientation = 'xy' | 'xz' | 'yz'
export type Tool = 'pointer' | 'pan' | 'brush' | 'eraser' | 'fill' | 'pick' | 'link' | 'cut'

export interface ChannelState {
  name: string
  visible: boolean
  window: [number, number]
  /** 0.20–3.00. */
  gamma: number
  /** CSS color. */
  color: string
}

export interface SliceViewState {
  /** Index along the orthogonal axis: z for xy, y for xz, x for yz. */
  index: number
  camera: { target: [number, number]; zoom: number }
}

export interface VolumeViewState {
  camera: {
    rotationX: number
    rotationOrbit: number
    zoom: number
    target: [number, number, number]
  }
}

export interface OverlayState {
  labels: { on: boolean; opacity: number }
  tracks: { on: boolean; opacity: number; trail: number }
}

/**
 * Display-only multipliers on top of the voxel-size aspect, global across views.
 * Never applied to reported coordinates, centroids, or picking.
 */
export interface AxisScale {
  z: number
  y: number
  x: number
}

export const AXIS_SCALE_MIN = 0.1
export const AXIS_SCALE_MAX = 10

export interface NavState {
  project: ProjectInfo | null
  /** One timeline: stepping t in any view steps it for the app. */
  t: number
  activeView: ActiveView
  slices: Record<SliceOrientation, SliceViewState>
  volume: VolumeViewState
  channels: ChannelState[]
  /** Last-clicked channel square: what the readout reports and settings target. */
  activeChannel: number
  overlays: OverlayState
  axisScale: AxisScale
  transport: { playing: 'off' | 't' | 'slice' }
  tool: Tool
  selection: { cellId: number } | null
  /** Bumped on every navigation change; results whose key is stale never commit. */
  generation: number

  initProject(project: ProjectInfo): void
  stepT(delta: number): void
  setT(t: number): void
  stepSlice(delta: number): void
  setSliceIndex(index: number): void
  setActiveView(view: ActiveView): void
  setChannel(index: number, patch: Partial<ChannelState>): void
  setActiveChannel(index: number): void
  toggleChannel(index: number): void
  showAllChannels(): void
  setOverlays(patch: Partial<OverlayState>): void
  setAxisScale(patch: Partial<AxisScale>): void
  resetAxisScale(): void
  setTool(tool: Tool): void
  setPlaying(playing: 'off' | 't' | 'slice'): void
  /** Stays in the current view unless `view` is given. */
  jumpTo(pose: { t?: number; z?: number; y?: number; x?: number; view?: ActiveView }): void
  jumpToCell(cell: CellRow, view?: ActiveView): void
  select(cellId: number | null): void
}

const DEFAULT_COLORS = ['#ff5c73', '#52df83', '#5ba7ff', '#d67cff', '#ffb100', '#4be0d3']

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v))

/** Which axis a slice view steps along, and the dimension that bounds it. */
export const sliceAxis = (o: SliceOrientation): 'z' | 'y' | 'x' =>
  o === 'xy' ? 'z' : o === 'xz' ? 'y' : 'x'

export function channelStateFrom(meta: ChannelMeta, index: number, dtypeMax: number): ChannelState {
  return {
    name: meta.name || `Channel ${index + 1}`,
    visible: index < 3,
    window: meta.window ?? [0, dtypeMax],
    gamma: 1,
    color: meta.color
      ? `#${meta.color}`
      : (DEFAULT_COLORS[index % DEFAULT_COLORS.length] as string),
  }
}

const emptySlice = (): SliceViewState => ({ index: 0, camera: { target: [0, 0], zoom: 0 } })

export const useNav = create<NavState>((set, get) => ({
  project: null,
  t: 0,
  activeView: 'xy',
  slices: { xy: emptySlice(), xz: emptySlice(), yz: emptySlice() },
  volume: { camera: { rotationX: 25, rotationOrbit: 25, zoom: 0, target: [0, 0, 0] } },
  channels: [],
  activeChannel: 0,
  overlays: {
    labels: { on: true, opacity: 0.36 },
    tracks: { on: true, opacity: 0.85, trail: 6 },
  },
  axisScale: { z: 1, y: 1, x: 1 },
  transport: { playing: 'off' },
  tool: 'pointer',
  selection: null,
  generation: 0,

  initProject(project) {
    const { dims, channels, dtype } = project
    const dtypeMax = dtype === 'u8' ? 255 : dtype === 'u16' ? 65535 : 4294967295
    set({
      project,
      t: 0,
      channels: channels.map((m, i) => channelStateFrom(m, i, dtypeMax)),
      activeChannel: 0,
      slices: {
        xy: { index: Math.floor(dims.z / 2), camera: { target: [0, 0], zoom: 0 } },
        xz: { index: Math.floor(dims.y / 2), camera: { target: [0, 0], zoom: 0 } },
        yz: { index: Math.floor(dims.x / 2), camera: { target: [0, 0], zoom: 0 } },
      },
      generation: get().generation + 1,
    })
  },

  stepT(delta) {
    get().setT(get().t + delta)
  },

  setT(t) {
    const max = (get().project?.dims.t ?? 1) - 1
    set({ t: clamp(Math.round(t), 0, Math.max(0, max)), generation: get().generation + 1 })
  },

  stepSlice(delta) {
    const { activeView } = get()
    if (activeView === '3d') return
    get().setSliceIndex(get().slices[activeView].index + delta)
  },

  setSliceIndex(index) {
    const { activeView, project, slices, generation } = get()
    if (activeView === '3d' || !project) return
    const axis = sliceAxis(activeView)
    const max = project.dims[axis] - 1
    set({
      slices: {
        ...slices,
        [activeView]: {
          ...slices[activeView],
          index: clamp(Math.round(index), 0, Math.max(0, max)),
        },
      },
      generation: generation + 1,
    })
  },

  setActiveView(view) {
    set({ activeView: view, generation: get().generation + 1 })
  },

  setChannel(index, patch) {
    const channels = get().channels.slice()
    const current = channels[index]
    if (!current) return
    channels[index] = { ...current, ...patch }
    set({ channels })
  },

  setActiveChannel(index) {
    if (index >= 0 && index < get().channels.length) set({ activeChannel: index })
  },

  toggleChannel(index) {
    const current = get().channels[index]
    if (!current) return
    get().setChannel(index, { visible: !current.visible })
    set({ activeChannel: index })
  },

  showAllChannels() {
    set({ channels: get().channels.map((c) => ({ ...c, visible: true })) })
  },

  setOverlays(patch) {
    set({ overlays: { ...get().overlays, ...patch } })
  },

  setAxisScale(patch) {
    const next = { ...get().axisScale, ...patch }
    set({
      axisScale: {
        z: clamp(next.z, AXIS_SCALE_MIN, AXIS_SCALE_MAX),
        y: clamp(next.y, AXIS_SCALE_MIN, AXIS_SCALE_MAX),
        x: clamp(next.x, AXIS_SCALE_MIN, AXIS_SCALE_MAX),
      },
    })
  },

  resetAxisScale() {
    set({ axisScale: { z: 1, y: 1, x: 1 } })
  },

  setTool(tool) {
    set({ tool })
  },

  setPlaying(playing) {
    set({ transport: { playing } })
  },

  jumpTo(pose) {
    const { project, slices, activeView, generation } = get()
    if (!project) return
    const view = pose.view ?? activeView
    const nextSlices = { ...slices }
    for (const o of ['xy', 'xz', 'yz'] as const) {
      const axis = sliceAxis(o)
      const value = pose[axis]
      if (value === undefined) continue
      nextSlices[o] = {
        ...nextSlices[o],
        index: clamp(Math.round(value), 0, Math.max(0, project.dims[axis] - 1)),
      }
    }
    if (pose.y !== undefined && pose.x !== undefined) {
      const target: [number, number] = [pose.x, pose.y]
      nextSlices.xy = { ...nextSlices.xy, camera: { ...nextSlices.xy.camera, target } }
    }
    set({
      t: pose.t !== undefined ? clamp(Math.round(pose.t), 0, project.dims.t - 1) : get().t,
      slices: nextSlices,
      activeView: view,
      generation: generation + 1,
    })
  },

  jumpToCell(cell, view) {
    const [z, y, x] = cell.centroid ?? [0, 0, 0]
    get().jumpTo({ t: cell.t, z, y, x, view })
    set({ selection: { cellId: cell.id } })
  },

  select(cellId) {
    set({ selection: cellId === null ? null : { cellId } })
  },
}))
