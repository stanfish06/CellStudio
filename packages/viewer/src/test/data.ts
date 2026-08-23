import type {
  Bbox,
  CellRow,
  DeleteMaskBody,
  Dims,
  LabelLease,
  LayerId,
  Level,
  MaskEditResult,
  PlaneBuffer,
  ProjectInfo,
  StrokeBody,
  VolumeBuffer,
} from '@cellstudio/api-client'
import type { MaskApi, OrthoAxis, PixelApi } from '../data/api'
import type { NavSnapshot } from '../scenes/types'
import {
  channelStateFrom,
  type ActiveView,
  type ChannelState,
  type OrbitCamera,
  type Tool,
} from '../state/nav'

const dims = (t: number, c: number, z: number, y: number, x: number): Dims => ({ t, c, z, y, x })

const level = (index: number, y: number, x: number, factor: [number, number, number]): Level => ({
  index,
  dims: dims(277, 3, 3, y, x),
  chunks: dims(1, 1, 3, y, x),
  factor,
})

/** The development dataset: TCZYX 277×3×3×1024×1024 uint16, XY-only 3-level pyramid. */
export const devProject = (overrides: Partial<ProjectInfo> = {}): ProjectInfo => ({
  sessionId: 'session-1',
  sourcePath: '/data/dev.zarr',
  projectPath: '/data/dev.zarr.cellstudio',
  dims: dims(277, 3, 3, 1024, 1024),
  dtype: 'u16',
  scale: { z: 2.0, y: 0.603, x: 0.603 },
  levels: [
    level(0, 1024, 1024, [1, 1, 1]),
    level(1, 512, 512, [1, 2, 2]),
    level(2, 256, 256, [1, 4, 4]),
  ],
  channels: [
    { name: 'membrane', color: 'ff0000', window: [100, 4000] },
    { name: 'nuclei', color: '00ff00', window: [50, 3000] },
    { name: 'reporter', color: '0000ff', window: [0, 2000] },
  ],
  versions: { sessionId: 'session-1', image: 1, labels: 1, graph: 1, settings: 1 },
  layout: { hostile: false, amplification: { xy: 3, xz: 1, yz: 1 }, affectedViews: [] },
  hasLabels: false,
  ...overrides,
})

export const channelsFor = (project: ProjectInfo): ChannelState[] =>
  project.channels.map((meta, i) => channelStateFrom(meta, i, 65535))

export interface NavOverrides {
  t?: number
  activeView?: ActiveView
  index?: Partial<Record<'xy' | 'xz' | 'yz', number>>
  zoom?: Partial<Record<'xy' | 'xz' | 'yz', number>>
  axisScale?: { z: number; y: number; x: number }
  brushRadius?: number
  channels?: ChannelState[]
  camera?: OrbitCamera | null
  generation?: number
  trail?: number
  selection?: { cellId: number } | null
  tool?: Tool
  labels?: { on: boolean; opacity: number }
}

export function navSnapshot(project: ProjectInfo, o: NavOverrides = {}): NavSnapshot {
  const slice = (name: 'xy' | 'xz' | 'yz', fallback: number) => ({
    index: o.index?.[name] ?? fallback,
    camera: { target: [0, 0] as [number, number], zoom: o.zoom?.[name] ?? 0 },
  })
  return {
    project,
    t: o.t ?? 0,
    activeView: o.activeView ?? 'xy',
    slices: {
      xy: slice('xy', Math.floor(project.dims.z / 2)),
      xz: slice('xz', Math.floor(project.dims.y / 2)),
      yz: slice('yz', Math.floor(project.dims.x / 2)),
    },
    volume: { camera: o.camera ?? null },
    channels: o.channels ?? channelsFor(project),
    activeChannel: 0,
    overlays: {
      labels: o.labels ?? { on: true, opacity: 0.36 },
      tracks: { on: true, opacity: 0.85, trail: o.trail ?? 6 },
    },
    brush: { radius: o.brushRadius ?? 8 },
    tool: o.tool ?? 'pointer',
    axisScale: o.axisScale ?? { z: 1, y: 1, x: 1 },
    selection: o.selection ?? null,
    generation: o.generation ?? 1,
  }
}

/** deck.gl types `Layer['props']` narrowly; layer-prop assertions read through this. */
export const layerProps = (layer: { props: object } | undefined): Record<string, unknown> =>
  (layer?.props ?? {}) as unknown as Record<string, unknown>

export const cell = (
  id: number,
  t: number,
  centroid: [number, number, number],
  trackId = id,
): CellRow => ({
  id,
  t,
  centroid,
  area: 100,
  confidence: 0.9,
  state: null,
  trackId,
  reviewed: false,
})

export const makePlane = (
  height: number,
  width: number,
  channels: number,
  level = 0,
): PlaneBuffer => ({
  shape: [height, width],
  channels,
  dtype: 'u16',
  level,
  data: new Uint16Array(height * width * channels).buffer,
})

export const makeVolume = (z: number, y: number, x: number, level = 2): VolumeBuffer => ({
  shape: [z, y, x],
  dtype: 'u16',
  level,
  data: new Uint16Array(z * y * x).buffer,
})

/** Label reads are u32 whatever the image dtype is (design M1). */
export const makeLabelPlane = (height: number, width: number, level = 0): PlaneBuffer => ({
  shape: [height, width],
  channels: 1,
  dtype: 'u32',
  level,
  data: new Uint32Array(height * width).buffer,
})

export const makeLabelVolume = (z: number, y: number, x: number, level = 2): VolumeBuffer => ({
  shape: [z, y, x],
  dtype: 'u32',
  level,
  data: new Uint32Array(z * y * x).buffer,
})

interface Pending<T> {
  resolve: (v: T) => void
  reject: (e: unknown) => void
  signal?: AbortSignal
}

export interface SliceCall {
  q: { layer: LayerId; axis: OrthoAxis; t: number; cs: number[]; pos: number; level: number }
  signal?: AbortSignal
}

export interface VolumeCall {
  q: { layer: LayerId; t: number; c: number; level: number }
  signal?: AbortSignal
}

/** Records every call and can hold responses open, for supersession and abort tests. */
export class FakeApi implements PixelApi, MaskApi {
  auto = true
  sliceCalls: SliceCall[] = []
  volumeCalls: VolumeCall[] = []
  pixelCalls: { layer: LayerId; t: number; c: number; z: number; y: number; x: number }[] = []
  cellCalls: { t0: number; t1: number }[] = []
  cells: CellRow[] = []
  pixelValue = 1234
  labelValue = 0
  /** Simulated round trip for `pixel`, in fake-timer milliseconds. */
  pixelLatencyMs = 0
  strokeCalls: StrokeBody[] = []
  deleteCalls: DeleteMaskBody[] = []
  reserveCalls: number[] = []
  editCalls: ('undo' | 'redo')[] = []
  /** Rows the next mask edit reports back, for the TrackSource patch path. */
  editedCells: CellRow[] = []
  sessionId = 'session-1'
  labelVersion = 1
  nextLabelId = 1000
  private pendingEdits: Pending<MaskEditResult>[] = []
  private pendingSlices: Pending<PlaneBuffer>[] = []
  private pendingVolumes: Pending<VolumeBuffer>[] = []
  private pendingPixels: Pending<number>[] = []

  slice(q: SliceCall['q'], signal?: AbortSignal): Promise<PlaneBuffer> {
    this.sliceCalls.push({ q, signal })
    const plane =
      q.layer === 'labels'
        ? makeLabelPlane(3, 1024, q.level)
        : makePlane(3, 1024, q.cs.length, q.level)
    if (this.auto) return Promise.resolve(plane)
    return new Promise<PlaneBuffer>((resolve, reject) => {
      this.pendingSlices.push({ resolve, reject, signal })
    })
  }

  volume(q: VolumeCall['q'], signal?: AbortSignal): Promise<VolumeBuffer> {
    this.volumeCalls.push({ q, signal })
    const volume =
      q.layer === 'labels'
        ? makeLabelVolume(3, 256, 256, q.level)
        : makeVolume(3, 256, 256, q.level)
    if (this.auto) return Promise.resolve(volume)
    return new Promise<VolumeBuffer>((resolve, reject) => {
      this.pendingVolumes.push({ resolve, reject, signal })
    })
  }

  pixel(q: {
    layer: LayerId
    t: number
    c: number
    z: number
    y: number
    x: number
  }): Promise<number> {
    this.pixelCalls.push({ layer: q.layer, t: q.t, c: q.c, z: q.z, y: q.y, x: q.x })
    const value = q.layer === 'labels' ? this.labelValue : this.pixelValue
    if (this.auto) {
      if (this.pixelLatencyMs === 0) return Promise.resolve(value)
      return new Promise<number>((resolve) => setTimeout(() => resolve(value), this.pixelLatencyMs))
    }
    return new Promise<number>((resolve, reject) => {
      this.pendingPixels.push({ resolve, reject })
    })
  }

  cellsWindow(q: { t0: number; t1: number; bbox?: Bbox }): Promise<CellRow[]> {
    this.cellCalls.push({ t0: q.t0, t1: q.t1 })
    return Promise.resolve(this.cells)
  }

  reserveLabels(count: number): Promise<LabelLease> {
    this.reserveCalls.push(count)
    const lease = { first: this.nextLabelId, count }
    this.nextLabelId += count
    return Promise.resolve(lease)
  }

  stroke(body: StrokeBody): Promise<MaskEditResult> {
    this.strokeCalls.push(body)
    return this.settleEdit(`stroke:${body.label}`)
  }

  deleteMask(body: DeleteMaskBody): Promise<MaskEditResult> {
    this.deleteCalls.push(body)
    return this.settleEdit(`delete:${body.label}`, [body.label])
  }

  undo(): Promise<MaskEditResult> {
    this.editCalls.push('undo')
    return this.settleEdit('undo')
  }

  redo(): Promise<MaskEditResult> {
    this.editCalls.push('redo')
    return this.settleEdit('redo')
  }

  /** Settles one held mask edit, so a test can order a response against its event. */
  settleEditAt(at: number, result?: Partial<MaskEditResult>): void {
    const pending = this.pendingEdits[at]
    if (!pending) throw new Error(`no pending edit at ${at}`)
    pending.resolve({ ...this.editResult(), ...result })
  }

  failEditAt(at: number, error: unknown = new Error('stroke failed')): void {
    const pending = this.pendingEdits[at]
    if (!pending) throw new Error(`no pending edit at ${at}`)
    pending.reject(error)
  }

  get openEdits(): number {
    return this.pendingEdits.length
  }

  private settleEdit(scope: string, removed: number[] = []): Promise<MaskEditResult> {
    this.labelVersion += 1
    const result = { ...this.editResult(), removed, chunks: [scope] }
    if (this.auto) return Promise.resolve(result)
    return new Promise<MaskEditResult>((resolve, reject) => {
      this.pendingEdits.push({ resolve, reject })
    })
  }

  private editResult(): MaskEditResult {
    return {
      seq: this.labelVersion,
      version: this.labelVersion,
      sessionId: this.sessionId,
      hasLabels: true,
      cells: this.editedCells,
      removed: [],
      chunks: [],
    }
  }

  get openSlices(): number {
    return this.pendingSlices.length
  }

  get openVolumes(): number {
    return this.pendingVolumes.length
  }

  sliceSignal(at: number): AbortSignal | undefined {
    return this.pendingSlices[at]?.signal
  }

  /** Settles one held slice response, oldest first by default. */
  settleSlice(at: number, plane?: PlaneBuffer): void {
    const pending = this.pendingSlices[at]
    if (!pending) throw new Error(`no pending slice at ${at}`)
    pending.resolve(plane ?? makePlane(3, 1024, 1))
  }

  settleVolume(at: number, volume?: VolumeBuffer): void {
    const pending = this.pendingVolumes[at]
    if (!pending) throw new Error(`no pending volume at ${at}`)
    pending.resolve(volume ?? makeVolume(3, 256, 256))
  }

  settlePixel(at: number, value?: number): void {
    const pending = this.pendingPixels[at]
    if (!pending) throw new Error(`no pending pixel at ${at}`)
    pending.resolve(value ?? this.pixelValue)
  }
}
