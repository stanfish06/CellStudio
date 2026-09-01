import { create } from 'zustand'
import type { CellRow, ChannelMeta, ProjectInfo } from '@cellstudio/api-client'
import type { PixelZYX } from '../data/world'

export type ActiveView = 'xy' | 'xz' | 'yz' | '3d'
export type SliceOrientation = 'xy' | 'xz' | 'yz'
export type Tool = 'pointer' | 'pan' | 'brush' | 'eraser' | 'fill' | 'pick' | 'link'

/** The armed half of a Link: parent captured with the session and graph version it was
 * armed under, validated again at completion. */
export interface PendingLink {
  parentId: number
  sessionId: string
  graphVersion: number
}

/** A pendingLink armed under another session or an older graph must not complete. */
export function pendingLinkStale(pending: PendingLink, project: ProjectInfo): boolean {
  return pending.sessionId !== project.sessionId || pending.graphVersion !== project.versions.graph
}

export interface ChannelState {
  name: string
  visible: boolean
  window: [number, number]
  gamma: number
  color: string
}

export interface SliceViewState {
  /** Index along the orthogonal axis: z for xy, y for xz, x for yz. */
  index: number
  camera: { target: [number, number]; zoom: number }
}

export interface OrbitCamera {
  rotationX: number
  rotationOrbit: number
  zoom: number
  target: PixelZYX
}

export interface VolumeViewState {
  camera: OrbitCamera | null
}

export interface BrushState {
  /** Stamp radius in level-0 dataset pixels along x; other axes scale by voxel size. */
  radius: number
}

export const BRUSH_RADIUS_MIN = 1
export const BRUSH_RADIUS_MAX = 200

export interface OverlayState {
  labels: { on: boolean; opacity: number }
  /**
   * `trail` is the backward window length in frames, `fade` the linear opacity decay,
   * `dotSize` the centroid radius in image pixels — it scales with the image, so a dot
   * keeps its size relative to the cells at every zoom.
   */
  tracks: { on: boolean; opacity: number; trail: number; dotSize: number; fade: TrackFadeState }
}

export interface TrackFadeState {
  on: boolean
  max: number
  min: number
}

// Aspects
export interface AxisScale {
  z: number
  y: number
  x: number
}

export const AXIS_SCALE_MIN = 0.1
export const AXIS_SCALE_MAX = 10

export interface NavState {
  project: ProjectInfo | null
  t: number
  activeView: ActiveView
  slices: Record<SliceOrientation, SliceViewState>
  volume: VolumeViewState
  channels: ChannelState[]
  activeChannel: number
  overlays: OverlayState
  brush: BrushState
  axisScale: AxisScale
  transport: { playing: 'off' | 't' | 'slice' }
  tool: Tool
  selection: { cellId: number } | null
  /** The selected trail edge, for cutting one link instead of a whole track. */
  selectedLink: { parent: number; child: number } | null
  pendingLink: PendingLink | null
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
  setBrushRadius(radius: number): void
  setLabelsVersion(version: number): void
  setGraphVersion(version: number): void
  setAxisScale(patch: Partial<AxisScale>): void
  resetAxisScale(): void
  setTool(tool: Tool): void
  setPlaying(playing: 'off' | 't' | 'slice'): void
  setVolumeCamera(camera: OrbitCamera): void
  resetVolumeCamera(): void
  jumpTo(pose: { t?: number; z?: number; y?: number; x?: number; view?: ActiveView }): void
  jumpToCell(cell: CellRow, view?: ActiveView): void
  select(cellId: number | null): void
  selectLink(link: { parent: number; child: number } | null): void
  armLink(): boolean
  cancelLink(): void
  completeLink(): void
  markGraphPresent(): void
}

const DEFAULT_COLORS = ['#ff5c73', '#52df83', '#5ba7ff', '#d67cff', '#ffb100', '#4be0d3']

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v))

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
  volume: { camera: null },
  channels: [],
  activeChannel: 0,
  overlays: {
    labels: { on: true, opacity: 0.36 },
    tracks: {
      on: true,
      opacity: 0.85,
      trail: 10,
      dotSize: 3,
      fade: { on: true, max: 1, min: 0.15 },
    },
  },
  brush: { radius: 8 },
  axisScale: { z: 1, y: 1, x: 1 },
  transport: { playing: 'off' },
  tool: 'pointer',
  selection: null,
  selectedLink: null,
  pendingLink: null,
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
      volume: { camera: null },
      selection: null,
      selectedLink: null,
      tool: 'pointer',
      pendingLink: null,
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

  setBrushRadius(radius) {
    set({ brush: { radius: clamp(Math.round(radius), BRUSH_RADIUS_MIN, BRUSH_RADIUS_MAX) } })
  },

  /** A committed mask edit, without a `generation` bump: only the label fetch reruns. */
  setLabelsVersion(version) {
    const project = get().project
    if (!project || project.versions.labels >= version) return
    set({ project: { ...project, versions: { ...project.versions, labels: version } } })
  },

  /** A committed graph edit, without a `generation` bump: identity recolors, no image refetch. */
  setGraphVersion(version) {
    const project = get().project
    if (!project || project.versions.graph >= version) return
    set({ project: { ...project, versions: { ...project.versions, graph: version } } })
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
    if (tool === 'link') {
      if (get().armLink()) set({ tool })
      return
    }
    const { pendingLink } = get()
    if (pendingLink) set({ tool, pendingLink: null })
    else set({ tool })
  },

  setPlaying(playing) {
    set({ transport: { playing } })
  },

  setVolumeCamera(camera) {
    set({ volume: { camera } })
  },

  resetVolumeCamera() {
    if (get().volume.camera === null) return
    set({ volume: { camera: null } })
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
    set({ selection: { cellId: cell.id }, selectedLink: null })
  },

  select(cellId) {
    set({ selection: cellId === null ? null : { cellId }, selectedLink: null })
  },

  selectLink(link) {
    set({ selectedLink: link, selection: null })
  },

  /** Arms a link from the selected cell; refused without a graph or a selection. */
  armLink() {
    const { project, selection } = get()
    if (!project || !(project.hasGraph ?? false) || !selection) return false
    set({
      pendingLink: {
        parentId: selection.cellId,
        sessionId: project.sessionId,
        graphVersion: project.versions.graph,
      },
    })
    return true
  },

  /** Esc or an explicit cancel: disarm and leave the now-inert link tool. */
  cancelLink() {
    const { pendingLink, tool } = get()
    if (!pendingLink && tool !== 'link') return
    if (tool === 'link') set({ pendingLink: null, tool: 'pointer' })
    else set({ pendingLink: null })
  },

  /** A committed link: disarm and revert to the pointer. */
  completeLink() {
    set({ pendingLink: null, tool: 'pointer' })
  },

  /** The first graph edit or a finished import: enable Link/Unlink/save without a refetch. */
  markGraphPresent() {
    const project = get().project
    if (!project || project.hasGraph === true) return
    set({ project: { ...project, hasGraph: true } })
  },
}))
