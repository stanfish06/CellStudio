import { OrthographicView, type Layer, type PickingInfo } from '@deck.gl/core'
import { MultiscaleImageLayer } from '@hms-dbmi/viv'
import type { CellRow, Dims, PlaneBuffer } from '@cellstudio/api-client'
import { GpuBudget, gpuBudget as defaultBudget } from '../data/gpuBudget'
import { planeKeyId, type PlaneKey } from '../data/keys'
import type { PlaneCache } from '../data/planeCache'
import { DEFAULT_BRICK, LatestWins, nextBrickIndex, type BrickShape } from '../data/prefetch'
import type { TrackSource } from '../data/trackSource'
import {
  fitSlice,
  makeWorldTransform,
  sliceExtent,
  type Extent2D,
  type Fit,
  type Viewport2D,
  type WorldTransform,
} from '../data/world'
import type { XyPyramid } from '../data/xySource'
import { xySelections } from '../data/xySource'
import {
  OrthoPlaneLayer,
  hexToRgb,
  orthoPlaneExtensions,
  orthoPlaneProps,
} from '../layers/orthoPlane'
import { clampGamma } from '../layers/gamma'
import { TrackLayer, type TrackPoint } from '../layers/tracks'
import { vivLayer } from '../layers/viv'
import type { PerfMonitor } from '../perf'
import type { SliceOrientation } from '../state/nav'
import { Emitter, levelForZoom, visibleChannels, type NavSnapshot, type SceneStatus } from './types'

export interface SliceSceneOptions {
  orientation: SliceOrientation
  planes: PlaneCache
  tracks?: TrackSource
  perf?: PerfMonitor
  budget?: GpuBudget
  brick?: BrickShape
  /** Slab half-thickness for the track overlay, in pixels along the view normal. */
  slabRadius?: number
  id?: string
  /** Centroid pick; the session wires these to the nav store. */
  onSelect?: (cell: CellRow) => void
  onJumpToCell?: (cell: CellRow) => void
}

interface Committed {
  key: PlaneKey
  token: string
  plane: PlaneBuffer
}

/** deck.gl `OrthographicView` state: pan target in world units, log2 zoom. */
export interface SliceCamera {
  target: [number, number]
  zoom: number
}

const IDENTITY_UNIT = { z: 1, y: 1, x: 1 }

const sameCamera = (a: SliceCamera | null, b: SliceCamera | null): boolean =>
  a === b ||
  (a !== null &&
    b !== null &&
    a.zoom === b.zoom &&
    a.target[0] === b.target[0] &&
    a.target[1] === b.target[1])

/**
 * One slice view. XY reads the pyramid through viv's multiscale layer (pixel space, as
 * viv draws it); XZ/YZ draw one server-assembled quad in world units so the physical
 * aspect and per-axis display scaling apply.
 */
export class SliceScene {
  readonly orientation: SliceOrientation
  readonly id: string
  private readonly planes: PlaneCache
  private readonly tracksSource?: TrackSource
  private readonly perf?: PerfMonitor
  private readonly brick: BrickShape
  private readonly budget: GpuBudget
  private readonly slabRadius: number
  private readonly extensions = orthoPlaneExtensions()
  private readonly changed = new Emitter()
  private readonly onSelect?: (cell: CellRow) => void
  private readonly onJumpToCell?: (cell: CellRow) => void
  private pyramid: XyPyramid | null = null
  private viewportSize: Viewport2D = { width: 1024, height: 1024 }
  private nav: NavSnapshot | null = null
  private transform: WorldTransform = makeWorldTransform(null, IDENTITY_UNIT)
  private renderTransform: WorldTransform = this.transform
  private committed: Committed | null = null
  private latest = new LatestWins()
  private pendingToken: string | null = null
  private lastIndex: number | null = null
  private direction: 1 | -1 = 1
  private camera: SliceCamera | null = null
  private navCamera: SliceCamera | null = null
  private unsubscribeTracks?: () => void

  constructor(opts: SliceSceneOptions) {
    this.orientation = opts.orientation
    this.id = opts.id ?? `slice-${opts.orientation}`
    this.planes = opts.planes
    this.tracksSource = opts.tracks
    this.perf = opts.perf
    this.brick = opts.brick ?? DEFAULT_BRICK
    this.budget = opts.budget ?? defaultBudget
    this.slabRadius = opts.slabRadius ?? 2
    this.onSelect = opts.onSelect
    this.onJumpToCell = opts.onJumpToCell
    if (this.tracksSource) {
      this.unsubscribeTracks = this.tracksSource.onChange(() => this.changed.emit())
    }
  }

  onChange(cb: () => void): () => void {
    return this.changed.on(cb)
  }

  /** XY only: viv pixel sources over the raw `/store` passthrough. */
  setPyramid(pyramid: XyPyramid | null): void {
    this.pyramid = pyramid
    this.changed.emit()
  }

  setViewport(viewport: Viewport2D): void {
    this.viewportSize = viewport
  }

  /**
   * Live camera from deck's `onViewStateChange`. The frozen nav store has no setter for
   * it, so the pyramid level and the HUD zoom read it from here; a nav jump clears it.
   */
  setCamera(camera: SliceCamera | null): void {
    this.camera = camera
    this.changed.emit()
  }

  get scrubDirection(): 1 | -1 {
    return this.direction
  }

  /** The plane currently on screen, or null while nothing has committed. */
  get plane(): PlaneBuffer | null {
    return this.committed?.plane ?? null
  }

  get worldTransform(): WorldTransform {
    return this.transform
  }

  update(nav: NavSnapshot): void {
    const project = nav.project
    if (!project) return
    this.nav = nav
    this.transform = makeWorldTransform(project.scale, nav.axisScale)
    // viv's image layers draw XY in pixel space, so display scaling never reaches it.
    this.renderTransform =
      this.orientation === 'xy' ? makeWorldTransform(null, IDENTITY_UNIT) : this.transform

    const navCamera = nav.slices[this.orientation].camera
    if (!sameCamera(navCamera, this.navCamera)) {
      this.navCamera = { target: [...navCamera.target], zoom: navCamera.zoom }
      this.camera = null
    }

    const index = nav.slices[this.orientation].index
    if (this.lastIndex !== null && index !== this.lastIndex) {
      this.direction = index > this.lastIndex ? 1 : -1
    }
    this.lastIndex = index

    if (nav.overlays.tracks.on) this.tracksSource?.ensure(nav.t, nav.overlays.tracks.trail)

    if (this.orientation === 'xy') this.warmXyBrick(nav, project.dims)
    else this.requestPlane(nav)
  }

  /** Deck view for this scene; the ui supplies the controller-driven view state. */
  view(): OrthographicView {
    return new OrthographicView({ id: this.id, controller: true })
  }

  extent(): Extent2D {
    const dims = this.nav?.project?.dims
    if (!dims) return { width: 1, height: 1, pixelWidth: 1, pixelHeight: 1 }
    return sliceExtent(this.orientation, dims, this.renderTransform)
  }

  fit(): Fit {
    return fitSlice(this.extent(), this.viewportSize)
  }

  /** The live camera, then nav's when it has been moved, then the fit (nav starts at zoom 0). */
  viewState(): { target: [number, number, number]; zoom: number } {
    const camera = this.camera ?? this.nav?.slices[this.orientation].camera
    if (!camera || (camera.zoom === 0 && camera.target[0] === 0 && camera.target[1] === 0)) {
      const fit = this.fit()
      return { target: [fit.target[0], fit.target[1], 0], zoom: fit.zoom }
    }
    return { target: [camera.target[0], camera.target[1], 0], zoom: camera.zoom }
  }

  /** What the stage HUD reports, plus whether the current nav key is still in flight. */
  status(): SceneStatus {
    const nav = this.nav
    const zoom = this.viewState().zoom
    const level =
      this.orientation === 'xy'
        ? levelForZoom(zoom, nav?.project?.levels ?? [])
        : (this.committed?.key.level ?? levelForZoom(zoom, nav?.project?.levels ?? []))
    return { display: { level, zoom }, awaitingFrame: this.pendingToken !== null }
  }

  layers(): Layer[] {
    const nav = this.nav
    if (!nav?.project) return []
    const out: Layer[] = []
    const image = this.orientation === 'xy' ? this.xyLayer(nav) : this.orthoLayer(nav)
    if (image) out.push(image)
    const overlay = this.trackLayer(nav)
    if (overlay) out.push(overlay)
    return out
  }

  /**
   * Centroid picking on the track overlay. This is the centroid-only path — a detection
   * with no label voxel is still selectable because deck's pick radius covers the marker.
   * Selecting by label voxel goes through the readout's `labelId`, which is the cell id.
   */
  handlePick(info: PickingInfo, doubleClick = false): CellRow | null {
    const point = info.object as TrackPoint | undefined
    if (!point || typeof point.cellId !== 'number') return null
    const cell = this.tracksSource?.cell(point.cellId)
    if (!cell) return null
    if (doubleClick) this.onJumpToCell?.(cell)
    else this.onSelect?.(cell)
    return cell
  }

  /** Called after the frame containing the committed data has painted. */
  markPresented(): void {
    if (this.committed) this.perf?.presented(this.committed.token)
    this.perf?.frame()
  }

  /** Drops everything the old backend session put here; the next update refetches. */
  reset(): void {
    this.latest.abort()
    this.committed = null
    this.pendingToken = null
    this.pyramid = null
    this.nav = null
    this.camera = null
    this.navCamera = null
    this.lastIndex = null
    this.changed.emit()
  }

  dispose(): void {
    this.latest.abort()
    this.unsubscribeTracks?.()
    this.changed.clear()
  }

  private orthoAxis(): 'xz' | 'yz' {
    return this.orientation === 'xz' ? 'xz' : 'yz'
  }

  /** Zoom the fetch level comes from: the live camera when the ui set one, nav's otherwise. */
  private fetchCamera(nav: NavSnapshot): SliceCamera {
    return this.camera ?? nav.slices[this.orientation].camera
  }

  private planeKey(nav: NavSnapshot): PlaneKey | null {
    const project = nav.project
    if (!project || this.orientation === 'xy') return null
    const channels = visibleChannels(nav.channels).map((c) => c.index)
    if (channels.length === 0) return null
    return {
      layer: 'image',
      axis: this.orthoAxis(),
      level: levelForZoom(this.fetchCamera(nav).zoom, project.levels),
      t: nav.t,
      c: channels,
      index: nav.slices[this.orientation].index,
      version: project.versions.image,
    }
  }

  private requestPlane(nav: NavSnapshot): void {
    const key = this.planeKey(nav)
    const project = nav.project
    if (!key || !project) return
    const token = `${planeKeyId(key)}#${nav.generation}`
    if (this.committed?.token === token) {
      this.pendingToken = null
      return
    }
    this.pendingToken = token
    this.perf?.begin('ortho-step', token)
    void this.latest
      .run(token, (signal) => this.planes.get(key, signal))
      .then((plane) => {
        if (!plane) {
          this.perf?.cancel(token)
          return
        }
        this.pendingToken = null
        this.committed = { key, token, plane }
        this.changed.emit()
      })
      .catch(() => {
        this.pendingToken = null
        this.perf?.cancel(token)
      })
    const axis = key.axis === 'xz' ? 'y' : 'x'
    this.planes.prefetch(key, this.direction, project.dims[axis] - 1)
  }

  /**
   * One warming read per brick along z: the store holds a whole z-brick per chunk, so a
   * single tile fetch puts every plane of the next brick in the browser HTTP cache.
   */
  private warmXyBrick(nav: NavSnapshot, dims: Dims): void {
    const pyramid = this.pyramid
    if (!pyramid) return
    const index = nav.slices.xy.index
    const z = nextBrickIndex(index, this.direction, this.brick.z, dims.z - 1)
    if (z === null) return
    const camera = this.fetchCamera(nav)
    const level = levelForZoom(camera.zoom, nav.project?.levels ?? [])
    const source = pyramid.levels[Math.min(level, pyramid.levels.length - 1)]
    if (!source) return
    const scale = 2 ** level * pyramid.tileSize
    const x = Math.max(0, Math.floor(camera.target[0] / scale))
    const y = Math.max(0, Math.floor(camera.target[1] / scale))
    for (const { index: c } of visibleChannels(nav.channels)) {
      void source.getTile({ x, y, selection: { t: nav.t, c, z } }).catch(() => {})
    }
  }

  private xyLayer(nav: NavSnapshot): Layer | null {
    const pyramid = this.pyramid
    if (!pyramid || !nav.project) return null
    const visible = visibleChannels(nav.channels)
    if (visible.length === 0) return null
    const token = `xy/${nav.t}/${nav.slices.xy.index}#${nav.generation}`
    this.perf?.begin('xy-step', token)
    return vivLayer(MultiscaleImageLayer, {
      id: `${this.id}-image`,
      loader: pyramid.levels,
      selections: xySelections(
        visible.map((v) => v.index),
        nav.t,
        nav.slices.xy.index,
      ),
      contrastLimits: visible.map((v) => v.state.window),
      channelsVisible: visible.map(() => true),
      colors: visible.map((v) => hexToRgb(v.state.color)),
      gammas: visible.map((v) => clampGamma(v.state.gamma)),
      extensions: this.extensions,
      opacity: 1,
      pickable: true,
      // What the volume left over, so a slice <-> 3D switch does not thrash.
      maxCacheSize: this.budget.tileCacheSize(pyramid.tileSize, visible.length),
      onViewportLoad: () => {
        this.perf?.presented(token)
        this.changed.emit()
      },
    })
  }

  private orthoLayer(nav: NavSnapshot): Layer | null {
    const committed = this.committed
    if (!committed || !nav.project) return null
    const extent = this.extent()
    const channels = committed.key.c
      .map((index) => nav.channels[index])
      .filter((c): c is NonNullable<typeof c> => c !== undefined)
    const props = orthoPlaneProps({
      id: `${this.id}-plane`,
      plane: committed.plane,
      bounds: [0, extent.height, extent.width, 0],
      channels,
      selections: committed.key.c.map((c) => ({
        t: committed.key.t,
        c,
        index: committed.key.index,
      })),
    })
    return vivLayer(OrthoPlaneLayer, { ...props, extensions: this.extensions })
  }

  private trackLayer(nav: NavSnapshot): Layer | null {
    const cells = this.tracksSource?.cells
    if (!cells || cells.length === 0 || !nav.overlays.tracks.on) return null
    return new TrackLayer({
      id: `${this.id}-tracks`,
      cells,
      t: nav.t,
      trail: nav.overlays.tracks.trail,
      transform: this.renderTransform,
      orientation: this.orientation,
      index: nav.slices[this.orientation].index,
      slab: this.slabRadius,
      trackOpacity: nav.overlays.tracks.opacity,
      lineage: nav.selection ? new Set([nav.selection.cellId]) : undefined,
    }) as unknown as Layer
  }
}
