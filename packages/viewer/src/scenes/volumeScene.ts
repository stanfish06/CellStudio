import {
  COORDINATE_SYSTEM,
  OrbitView,
  type Layer,
  type OrbitViewState,
  type PickingInfo,
} from '@deck.gl/core'
import { PathLayer } from '@deck.gl/layers'
import { XR3DLayer } from '@hms-dbmi/viv'
import type { CellRow, Level, VolumeBuffer } from '@cellstudio/api-client'
import { GpuBudget, gpuBudget as defaultBudget } from '../data/gpuBudget'
import { volumeKeyId, type VolumeKey } from '../data/keys'
import { LatestWins } from '../data/prefetch'
import { withHighlightSlots, type RemapCache } from '../data/trackFrame'
import type { TrackSource } from '../data/trackSource'
import type { LabelVolumeView, MaskEditor } from '../edit/maskEditor'
import {
  fitVolume,
  makeWorldTransform,
  volumeFrame,
  volumeExtent,
  type PixelZYX,
  type Viewport2D,
  type WorldTransform,
  type WorldXYZ,
} from '../data/world'
import type { VolumeCache } from '../data/volumeCache'
import { HIGHLIGHT_BASE, HIGHLIGHT_SLOTS, HIGHLIGHT_STRIDE } from '../layers/labelPalette'
import { LabelVolumeLayer, labelVolumeExtension, labelVolumeProps } from '../layers/labelVolume'
import {
  type LabelHighlight,
  type LineageOverlay,
  type Rgb,
  TrackLayer3D,
  type TrackPoint,
  type TrackSegment,
  highlightSlots,
  labeledCells,
} from '../layers/tracks'
import { labelHex } from '../layers/labelOutline'
import { hexToRgb } from '../layers/orthoPlane'
import { lineageHighlight } from './sliceScene'
import {
  gamma3DExtension,
  volumeProps,
  type RenderingMode,
  type VolumeChannel,
} from '../layers/volume'
import { vivLayer } from '../layers/viv'
import type { PerfMonitor } from '../perf'
import type { OrbitCamera } from '../state/nav'
import { Emitter, visibleChannels, type NavSnapshot, type SceneStatus } from './types'

const FIT_ROTATION = { rotationX: 25, rotationOrbit: 25 }

/** No base contains no acknowledged operation, so the first stroke's echo survives. */
const NO_BASE_VERSION = -1

const CURSOR_COLOR: [number, number, number, number] = [255, 255, 255, 220]

const RING_SEGMENTS = 32

/** World units the orb travels per wheel pixel — a trackpad flick is a stream of events. */
const ORB_WORLD_PER_PIXEL = 0.25

const clamp01 = (v: number) => Math.min(1, Math.max(0, v))

/**
 * Where the pointer ray crosses the volume's world box, as distances along it. A miss is
 * null: there is no paintable depth under a pointer that is not looking through the
 * volume. */
export function rayBoxInterval(
  origin: WorldXYZ,
  direction: WorldXYZ,
  box: WorldXYZ,
): { near: number; far: number } | null {
  let near = 0
  let far = Number.POSITIVE_INFINITY
  for (let i = 0; i < 3; i++) {
    const o = origin[i] as number
    const d = direction[i] as number
    const size = box[i] as number
    if (Math.abs(d) < 1e-12) {
      if (o < 0 || o > size) return null
      continue
    }
    const a = -o / d
    const b = (size - o) / d
    near = Math.max(near, Math.min(a, b))
    far = Math.min(far, Math.max(a, b))
  }
  return far >= near ? { near, far } : null
}

const levelFactorOf = (levels: readonly Level[], level: number): PixelZYX => {
  const [fz, fy, fx] = levels.find((l) => l.index === level)?.factor ?? [1, 1, 1]
  return [fz ?? 1, fy ?? 1, fx ?? 1]
}

export interface VolumeSceneOptions {
  volumes: VolumeCache
  labelVolumes?: VolumeCache
  editor?: MaskEditor
  tracks?: TrackSource
  /** Display copies of label volumes with voxel ids mapped to track ids. */
  remaps?: RemapCache
  perf?: PerfMonitor
  budget?: GpuBudget
  renderingMode?: RenderingMode
  /** World units the orb travels per wheel pixel. */
  orbStep?: number
  id?: string
  onSelect?: (cell: CellRow) => void
  onJumpToCell?: (cell: CellRow) => void
  /**A click while the link tool is armed, resolved to the target cell id*/
  onLinkTarget?: (cellId: number) => void
  /** A click on a trail segment: the edge it names, for cutting one link. */
  onSelectLink?: (link: { parent: number; child: number }) => void
}

interface Committed {
  token: string
  level: number
  t: number
  channels: { index: number; buffer: VolumeBuffer }[]
  labels: { buffer: VolumeBuffer; version: number } | null
}

export class VolumeScene {
  readonly id: string
  private readonly volumes: VolumeCache
  private readonly labelVolumes?: VolumeCache
  private readonly editor?: MaskEditor
  private readonly orbStep: number
  private readonly tracksSource?: TrackSource
  private readonly remaps?: RemapCache
  private readonly perf?: PerfMonitor
  private readonly budget: GpuBudget
  private readonly changed = new Emitter()
  private readonly onSelect?: (cell: CellRow) => void
  private readonly onJumpToCell?: (cell: CellRow) => void
  private readonly onLinkTarget?: (cellId: number) => void
  private readonly onSelectLink?: (link: { parent: number; child: number }) => void
  private lineage: LineageOverlay | null = null
  private renderingMode: RenderingMode
  private viewportSize: Viewport2D = { width: 1024, height: 1024 }
  private nav: NavSnapshot | null = null
  private transform: WorldTransform = makeWorldTransform(null, { z: 1, y: 1, x: 1 })
  private frame: WorldTransform = this.transform
  private committed: Committed | null = null
  private latest = new LatestWins()
  private level = 0
  private pendingToken: string | null = null
  private lastT: number | null = null
  private tDirection: 1 | -1 = 1
  private ray: { origin: WorldXYZ; direction: WorldXYZ } | null = null
  private interval: { near: number; far: number } | null = null
  private orb = 0.5
  private unsubscribeTracks?: () => void
  private unsubscribeEditor?: () => void

  constructor(opts: VolumeSceneOptions) {
    this.id = opts.id ?? 'volume'
    this.volumes = opts.volumes
    this.labelVolumes = opts.labelVolumes
    this.editor = opts.editor
    this.orbStep = opts.orbStep ?? ORB_WORLD_PER_PIXEL
    this.tracksSource = opts.tracks
    this.remaps = opts.remaps
    this.perf = opts.perf
    this.budget = opts.budget ?? defaultBudget
    this.renderingMode = opts.renderingMode ?? 'additive'
    this.onSelect = opts.onSelect
    this.onJumpToCell = opts.onJumpToCell
    this.onLinkTarget = opts.onLinkTarget
    this.onSelectLink = opts.onSelectLink
    if (this.tracksSource) {
      this.unsubscribeTracks = this.tracksSource.onChange(() => this.changed.emit())
    }
    if (this.editor) {
      this.unsubscribeEditor = this.editor.onChange(() => this.changed.emit())
    }
  }

  onChange(cb: () => void): () => void {
    return this.changed.on(cb)
  }

  setViewport(viewport: Viewport2D): void {
    this.viewportSize = viewport
  }

  setRenderingMode(mode: RenderingMode): void {
    this.renderingMode = mode
    this.changed.emit()
  }

  /**The selected lineage, replacing the one-element highlight stub*/
  setLineage(overlay: LineageOverlay | null): void {
    if (this.lineage === overlay) return
    this.lineage = overlay
    this.changed.emit()
  }

  get mode(): RenderingMode {
    return this.renderingMode
  }

  get volumeLevel(): number {
    return this.level
  }

  get volume(): Committed | null {
    return this.committed
  }

  view(): OrbitView {
    return new OrbitView({
      id: this.id,
      orbitAxis: 'Y',
      controller: {
        scrollZoom: true,
        dragPan: true,
        dragRotate: true,
        doubleClickZoom: true,
        touchZoom: true,
        touchRotate: false,
        // the arrow keys already step t
        keyboard: false,
        inertia: 0,
      },
    })
  }

  extent(): WorldXYZ {
    const dims = this.nav?.project?.dims
    if (!dims) return [1, 1, 1]
    return volumeExtent(dims, this.transform)
  }

  viewState(): OrbitViewState {
    const camera = this.nav?.volume.camera
    if (camera) {
      return {
        target: [...this.frame.toWorld(camera.target)],
        zoom: camera.zoom,
        rotationX: camera.rotationX,
        rotationOrbit: camera.rotationOrbit,
      }
    }
    const fit = fitVolume(this.extent(), this.viewportSize)
    return { ...FIT_ROTATION, zoom: fit.zoom, target: [...fit.target3d] }
  }

  cameraFrom(next: OrbitViewState): OrbitCamera {
    const camera = this.pose(next)
    const values = [camera.rotationX, camera.rotationOrbit, camera.zoom, ...camera.target]
    return values.every(Number.isFinite) ? camera : this.pose(this.viewState())
  }

  update(nav: NavSnapshot): void {
    const project = nav.project
    if (!project) return
    this.nav = nav
    this.transform = makeWorldTransform(project.scale, nav.axisScale)
    this.frame = volumeFrame(this.transform, project.dims)
    if (this.lastT !== null && nav.t !== this.lastT) this.tDirection = nav.t > this.lastT ? 1 : -1
    this.lastT = nav.t

    const visible = visibleChannels(nav.channels)
    if (visible.length === 0) return
    const labels = nav.overlays.labels.on && (this.editor?.labelsPresent ?? false)
    const labelCache = labels ? this.labelVolumes : undefined
    // The label volume costs a channel of the ceiling, so both volumes drop a level
    // together and stay aligned voxel for voxel.
    const plan = this.budget.planVolume(
      project.levels,
      visible.length + (labelCache ? 1 : 0),
      project.dtype,
    )
    this.level = plan.level
    this.recomputeInterval()
    this.volumes.configure({
      layer: 'image',
      level: plan.level,
      version: project.versions.image,
      channels: visible.map((v) => v.index),
      tMax: project.dims.t - 1,
    })

    // Labels need graph identity too: hiding trails must not revert mask colors —
    // but only when the project actually has a graph (`hasGraph`, not the labels proxy).
    const needsGraph = nav.overlays.tracks.on || (labels && (project.hasGraph ?? false))
    if (needsGraph) {
      this.tracksSource?.ensure(nav.t, nav.overlays.tracks.trail, project.versions.graph)
    }

    const keys: VolumeKey[] = visible.map((v) => ({
      layer: 'image',
      level: plan.level,
      t: nav.t,
      c: v.index,
      version: project.versions.image,
    }))
    const labelKey: VolumeKey | null = labelCache
      ? {
          layer: 'labels',
          level: plan.level,
          t: nav.t,
          c: 0,
          version: project.versions.labels,
        }
      : null
    if (labelCache && labelKey) {
      labelCache.configure({
        layer: 'labels',
        level: labelKey.level,
        version: labelKey.version,
        channels: [0],
        tMax: project.dims.t - 1,
      })
    }
    // Whether the overlay is on, whether a store exists and its version are not in the
    // image keys, so without them a toggle over a warm volume would request nothing.
    const token = `${keys.map(volumeKeyId).join('|')}+${
      labelKey ? volumeKeyId(labelKey) : 'no-labels'
    }#${nav.generation}`
    if (this.committed?.token === token) {
      this.pendingToken = null
      return
    }
    if (this.pendingToken === token) return
    this.pendingToken = token
    this.perf?.begin('t-step-3d', token)
    void this.latest
      .run(token, (signal) =>
        Promise.all([
          Promise.all(keys.map((k) => this.volumes.get(k, signal))),
          labelCache && labelKey ? labelCache.get(labelKey, signal) : Promise.resolve(null),
        ]),
      )
      .then((result) => {
        if (!result) {
          this.perf?.cancel(token)
          return
        }
        const [buffers, labelBuffer] = result
        this.pendingToken = null
        this.committed = {
          token,
          level: plan.level,
          t: nav.t,
          channels: buffers.map((buffer, i) => ({
            index: keys[i]?.c ?? 0,
            buffer,
          })),
          labels:
            labelBuffer && labelKey ? { buffer: labelBuffer, version: labelKey.version } : null,
        }
        this.changed.emit()
      })
      .catch(() => {
        this.pendingToken = null
        this.perf?.cancel(token)
      })
    this.volumes.prefetch(nav.t + this.tDirection)
    if (labelCache) labelCache.prefetch(nav.t + this.tDirection)
  }

  /**
   * The pointer ray in world space. The orb rides a normalized parameter over the ray's
   * span through the volume, so `u` survives a camera, viewport or scale change and a
   * miss leaves no paintable cursor at all.   */
  setPointerRay(ray: { origin: WorldXYZ; direction: WorldXYZ } | null): void {
    const before = this.orbWorld()
    this.ray = ray
    this.recomputeInterval()
    const after = this.orbWorld()
    const same =
      (before === null && after === null) ||
      (before !== null && after !== null && before.every((v, i) => v === after[i]))
    if (!same) this.changed.emit()
  }

  /** Wheel pixels, not events: one voxel per event jumps tens of voxels per trackpad flick. */
  stepOrbU(pixelDelta: number): void {
    const interval = this.interval
    if (!interval) return
    const span = interval.far - interval.near
    if (!(span > 0)) return
    const next = clamp01(this.orb + (pixelDelta * this.orbStep) / span)
    if (next === this.orb) return
    this.orb = next
    this.changed.emit()
  }

  /** Normalized depth along the pointer ray's span through the volume. */
  get orbU(): number {
    return this.orb
  }

  /** True while the pointer ray crosses the volume; false is "paints nothing". */
  get orbActive(): boolean {
    return this.interval !== null
  }

  orbWorld(): WorldXYZ | null {
    const { ray, interval } = this
    if (!ray || !interval) return null
    const d = interval.near + this.orb * (interval.far - interval.near)
    return [
      (ray.origin[0] as number) + (ray.direction[0] as number) * d,
      (ray.origin[1] as number) + (ray.direction[1] as number) * d,
      (ray.origin[2] as number) + (ray.direction[2] as number) * d,
    ]
  }

  /** The orb centre in dataset voxels — through `fromWorld`, so `axisScale` never reaches
   * the stamp. */
  orbCentre(): PixelZYX | null {
    const world = this.orbWorld()
    return world ? this.frame.fromWorld(world) : null
  }

  private recomputeInterval(): void {
    this.interval = this.ray
      ? rayBoxInterval(this.ray.origin, this.ray.direction, this.extent())
      : null
  }

  status(): SceneStatus {
    return {
      display: { level: this.level, zoom: this.viewState().zoom },
      awaitingFrame: this.pendingToken !== null,
    }
  }

  layers(): Layer[] {
    const nav = this.nav
    if (!nav?.project) return []
    const out: Layer[] = []
    const volume = this.volumeLayer(nav)
    if (volume) out.push(volume)
    const labels = this.labelLayer(nav)
    if (labels) out.push(labels)
    const orb = this.orbLayer(nav)
    if (orb) out.push(orb)
    const cells = this.tracksSource?.cells
    if (cells && cells.length > 0 && nav.overlays.tracks.on) {
      const { set, overlay } = lineageHighlight(this.lineage, nav.selection?.cellId ?? null)
      out.push(
        new TrackLayer3D({
          id: `${this.id}-tracks`,
          cells,
          // overlays follow the pixels: the committed volume's frame, not nav's
          t: this.committed?.t ?? nav.t,
          highlighted: labeledCells(cells, this.committed?.t ?? nav.t, this.labelHighlights(nav)),
          trail: nav.overlays.tracks.trail,
          dotSize: nav.overlays.tracks.dotSize,
          selectedLink: nav.selectedLink,
          transform: this.frame,
          trackOpacity: nav.overlays.tracks.opacity,
          fade: nav.overlays.tracks.fade,
          lineage: set,
          lineageOverlay: overlay,
          // the volume writes depth, so an overlay inside the box is hidden unless the
          // depth comparison always passes (luma 9 names; `depthTest` is a v8 no-op)
          parameters: { depthCompare: 'always', depthWriteEnabled: false },
        }) as unknown as Layer,
      )
    }
    return out
  }

  handlePick(info: PickingInfo, jump = false): CellRow | null {
    // a trail pick names an edge, not a cell: the segment carries both endpoints
    const segment = info.object as TrackSegment | undefined
    if (!jump && segment && typeof segment.fromCellId === 'number' && this.onSelectLink) {
      this.onSelectLink({ parent: segment.fromCellId, child: segment.toCellId })
      return null
    }
    const point = info.object as TrackPoint | undefined
    if (!point || typeof point.cellId !== 'number') return null
    const cell = this.tracksSource?.cell(point.cellId)
    if (!cell) return null
    // An armed link claims the click; only centroids are pickable in 3D.
    if (!jump && this.nav?.tool === 'link' && this.nav.pendingLink && this.onLinkTarget) {
      this.onLinkTarget(cell.id)
      return cell
    }
    if (jump) this.onJumpToCell?.(cell)
    else this.onSelect?.(cell)
    return cell
  }

  markPresented(): void {
    if (this.committed) this.perf?.presented(this.committed.token)
    this.perf?.frame()
  }

  reset(): void {
    this.latest.abort()
    this.committed = null
    this.pendingToken = null
    this.lineage = null
    this.nav = null
    this.lastT = null
    this.level = 0
    this.ray = null
    this.interval = null
    this.orb = 0.5
    this.changed.emit()
  }

  dispose(): void {
    this.latest.abort()
    this.unsubscribeTracks?.()
    this.unsubscribeEditor?.()
    this.changed.clear()
  }

  private pose(state: OrbitViewState): OrbitCamera {
    return {
      rotationX: state.rotationX ?? FIT_ROTATION.rotationX,
      rotationOrbit: state.rotationOrbit ?? FIT_ROTATION.rotationOrbit,
      zoom: state.zoom,
      target: this.frame.fromWorld(state.target),
    }
  }

  private levelUnit(levels: readonly Level[], level: number): WorldXYZ {
    const found = levels.find((l) => l.index === level)
    // `factor` is [z, y, x] to match Dims and centroid order.
    const [fz, fy, fx] = found?.factor ?? [1, 1, 1]
    const unit = this.transform.unit
    return [unit[0] * fx, unit[1] * fy, unit[2] * fz]
  }

  private labelLayer(nav: NavSnapshot): Layer | null {
    const project = nav.project
    if (!project || !nav.overlays.labels.on) return null
    const committed = this.committed
    const base = committed?.labels ?? null
    const level = committed?.level ?? this.level
    const dims = project.levels.find((l) => l.index === level)?.dims ?? project.dims
    const view: LabelVolumeView = {
      t: committed?.t ?? nav.t,
      factor: levelFactorOf(project.levels, level),
      dims: [dims.z, dims.y, dims.x],
      level,
      version: base?.version ?? NO_BASE_VERSION,
    }
    if (view.t !== nav.t) return null
    const volume = this.editor
      ? this.editor.volumeBuffer(view, base?.buffer ?? null)
      : (base?.buffer ?? null)
    if (!volume) return null
    const {
      volume: shown,
      selectedLabel,
      highlightColors,
    } = this.displayVolume(nav, volume, base, view)
    return vivLayer(LabelVolumeLayer, {
      ...labelVolumeProps({
        id: `${this.id}-labels`,
        volume: shown,
        unit: this.levelUnit(project.levels, level),
        t: view.t,
        opacity: nav.overlays.labels.opacity,
        selectedLabel,
        highlightColors,
      }),
      extensions: [labelVolumeExtension()],
    })
  }

  /**
   * The display remap of D4, mirroring `SliceScene.displayPlane` for the label volume. Cells
   * carrying a highlighted label are redirected to highlight slots here, so the volume paints
   * them solid in the label's colour: a 3D boundary has no silhouette to trace, a filled cell
   * reads at any angle.
   */
  private displayVolume(
    nav: NavSnapshot,
    volume: VolumeBuffer,
    base: { buffer: VolumeBuffer; version: number } | null,
    view: LabelVolumeView,
  ): { volume: VolumeBuffer; selectedLabel: number; highlightColors: Rgb[] } {
    const selected = nav.selection?.cellId ?? 0
    const frame = this.tracksSource?.frame()
    const remap =
      this.remaps !== undefined &&
      frame !== undefined &&
      frame.ready &&
      (this.tracksSource?.cells.length ?? 0) > 0
    if (!remap || !this.remaps || !frame) {
      return { volume, selectedLabel: selected, highlightColors: [] }
    }
    const { slots, colors, signature } = highlightSlots(
      this.tracksSource?.cells ?? [],
      view.t,
      this.labelHighlights(nav),
      HIGHLIGHT_SLOTS,
    )
    const shownFrame = withHighlightSlots(frame, slots, HIGHLIGHT_BASE, HIGHLIGHT_STRIDE)
    const baseKey = base
      ? volumeKeyId({ layer: 'labels', level: view.level, t: view.t, c: 0, version: base.version })
      : `${this.id}-echo`
    const bufferKey = slots.size > 0 ? `${baseKey}|hl:${signature}` : baseKey
    return {
      volume: this.remaps.volume(bufferKey, volume, shownFrame, volume === base?.buffer),
      selectedLabel: shownFrame.trackIdFor(selected) ?? selected,
      highlightColors: colors,
    }
  }

  /** The sheet's highlighted labels with their colours, in sheet order. */
  private labelHighlights(nav: NavSnapshot): LabelHighlight[] {
    const defs = nav.project?.labelDefinitions ?? []
    return nav.overlays.highlightLabels.map((name) => ({
      name,
      color: hexToRgb(labelHex(defs.find((d) => d.name === name) ?? { name })),
    }))
  }

  /**
   * The orb the next press would stamp, as three rings. Depth testing is off: viv's
   * ray-cast volume writes no depth, so true occlusion is not available.   */
  private orbLayer(nav: NavSnapshot): Layer | null {
    if (nav.tool !== 'brush' && nav.tool !== 'eraser') return null
    const centre = this.orbWorld()
    if (!centre) return null
    const [cx, cy, cz] = centre
    const r = nav.brush.radius
    // The stamp is round in physical space; display scaling stretches only what is drawn.
    const rx = r
    const ry = (r * nav.axisScale.y) / nav.axisScale.x
    const rz = (r * nav.axisScale.z) / nav.axisScale.x
    const ring = (axes: 'xy' | 'xz' | 'yz'): WorldXYZ[] => {
      const path: WorldXYZ[] = []
      for (let i = 0; i <= RING_SEGMENTS; i++) {
        const a = (i / RING_SEGMENTS) * Math.PI * 2
        const c = Math.cos(a)
        const s = Math.sin(a)
        if (axes === 'xy') path.push([cx + rx * c, cy + ry * s, cz])
        else if (axes === 'xz') path.push([cx + rx * c, cy, cz + rz * s])
        else path.push([cx, cy + ry * c, cz + rz * s])
      }
      return path
    }
    return new PathLayer({
      id: `${this.id}-brush-cursor`,
      data: [ring('xy'), ring('xz'), ring('yz')],
      coordinateSystem: COORDINATE_SYSTEM.CARTESIAN,
      widthUnits: 'pixels',
      widthMinPixels: 1,
      pickable: false,
      parameters: { depthCompare: 'always', depthWriteEnabled: false },
      getPath: (p: WorldXYZ[]) => p,
      getWidth: 1.5,
      getColor: CURSOR_COLOR,
      updateTriggers: { getPath: [cx, cy, cz, rx, ry, rz] },
    }) as unknown as Layer
  }

  private volumeLayer(nav: NavSnapshot): Layer | null {
    const committed = this.committed
    if (!committed || !nav.project) return null
    const channels: VolumeChannel[] = []
    for (const { index, buffer } of committed.channels) {
      const channel = nav.channels[index]
      if (channel) channels.push({ index, buffer, channel })
    }
    if (channels.length === 0) return null
    const props = volumeProps({
      id: `${this.id}-volume`,
      channels,
      unit: this.levelUnit(nav.project.levels, committed.level),
      t: committed.t,
    })
    return vivLayer(XR3DLayer, {
      ...props,
      extensions: [gamma3DExtension(this.renderingMode)],
    })
  }
}
