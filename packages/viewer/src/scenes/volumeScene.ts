import { OrbitView, type Layer, type OrbitViewState, type PickingInfo } from '@deck.gl/core'
import { XR3DLayer } from '@hms-dbmi/viv'
import type { CellRow, Level, VolumeBuffer } from '@cellstudio/api-client'
import { GpuBudget, gpuBudget as defaultBudget } from '../data/gpuBudget'
import { volumeKeyId, type VolumeKey } from '../data/keys'
import { LatestWins } from '../data/prefetch'
import type { TrackSource } from '../data/trackSource'
import {
  fitVolume,
  makeWorldTransform,
  volumeExtent,
  type Viewport2D,
  type WorldTransform,
  type WorldXYZ,
} from '../data/world'
import type { VolumeCache } from '../data/volumeCache'
import { TrackLayer3D, type TrackPoint } from '../layers/tracks'
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

export interface VolumeSceneOptions {
  volumes: VolumeCache
  tracks?: TrackSource
  perf?: PerfMonitor
  budget?: GpuBudget
  renderingMode?: RenderingMode
  id?: string
  onSelect?: (cell: CellRow) => void
  onJumpToCell?: (cell: CellRow) => void
}

interface Committed {
  token: string
  level: number
  t: number
  channels: { index: number; buffer: VolumeBuffer }[]
}

export class VolumeScene {
  readonly id: string
  private readonly volumes: VolumeCache
  private readonly tracksSource?: TrackSource
  private readonly perf?: PerfMonitor
  private readonly budget: GpuBudget
  private readonly changed = new Emitter()
  private readonly onSelect?: (cell: CellRow) => void
  private readonly onJumpToCell?: (cell: CellRow) => void
  private renderingMode: RenderingMode
  private viewportSize: Viewport2D = { width: 1024, height: 1024 }
  private nav: NavSnapshot | null = null
  private transform: WorldTransform = makeWorldTransform(null, { z: 1, y: 1, x: 1 })
  private committed: Committed | null = null
  private latest = new LatestWins()
  private level = 0
  private pendingToken: string | null = null
  private lastT: number | null = null
  private tDirection: 1 | -1 = 1
  private unsubscribeTracks?: () => void

  constructor(opts: VolumeSceneOptions) {
    this.id = opts.id ?? 'volume'
    this.volumes = opts.volumes
    this.tracksSource = opts.tracks
    this.perf = opts.perf
    this.budget = opts.budget ?? defaultBudget
    this.renderingMode = opts.renderingMode ?? 'additive'
    this.onSelect = opts.onSelect
    this.onJumpToCell = opts.onJumpToCell
    if (this.tracksSource) {
      this.unsubscribeTracks = this.tracksSource.onChange(() => this.changed.emit())
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
        target: [...this.transform.toWorld(camera.target)],
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
    if (this.lastT !== null && nav.t !== this.lastT) this.tDirection = nav.t > this.lastT ? 1 : -1
    this.lastT = nav.t

    const visible = visibleChannels(nav.channels)
    if (visible.length === 0) return
    const plan = this.budget.planVolume(project.levels, visible.length, project.dtype)
    this.level = plan.level
    this.volumes.configure({
      layer: 'image',
      level: plan.level,
      version: project.versions.image,
      channels: visible.map((v) => v.index),
      tMax: project.dims.t - 1,
    })

    if (nav.overlays.tracks.on) this.tracksSource?.ensure(nav.t, nav.overlays.tracks.trail)

    const keys: VolumeKey[] = visible.map((v) => ({
      layer: 'image',
      level: plan.level,
      t: nav.t,
      c: v.index,
      version: project.versions.image,
    }))
    const token = `${keys.map(volumeKeyId).join('|')}#${nav.generation}`
    if (this.committed?.token === token) {
      this.pendingToken = null
      return
    }
    if (this.pendingToken === token) return
    this.pendingToken = token
    this.perf?.begin('t-step-3d', token)
    void this.latest
      .run(token, (signal) => Promise.all(keys.map((k) => this.volumes.get(k, signal))))
      .then((buffers) => {
        if (!buffers) {
          this.perf?.cancel(token)
          return
        }
        this.pendingToken = null
        this.committed = {
          token,
          level: plan.level,
          t: nav.t,
          channels: buffers.map((buffer, i) => ({
            index: keys[i]?.c ?? 0,
            buffer,
          })),
        }
        this.changed.emit()
      })
      .catch(() => {
        this.pendingToken = null
        this.perf?.cancel(token)
      })
    this.volumes.prefetch(nav.t + this.tDirection)
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
    const cells = this.tracksSource?.cells
    if (cells && cells.length > 0 && nav.overlays.tracks.on) {
      out.push(
        new TrackLayer3D({
          id: `${this.id}-tracks`,
          cells,
          t: nav.t,
          trail: nav.overlays.tracks.trail,
          transform: this.transform,
          trackOpacity: nav.overlays.tracks.opacity,
          lineage: nav.selection ? new Set([nav.selection.cellId]) : undefined,
        }) as unknown as Layer,
      )
    }
    return out
  }

  handlePick(info: PickingInfo, jump = false): CellRow | null {
    const point = info.object as TrackPoint | undefined
    if (!point || typeof point.cellId !== 'number') return null
    const cell = this.tracksSource?.cell(point.cellId)
    if (!cell) return null
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
    this.nav = null
    this.lastT = null
    this.level = 0
    this.changed.emit()
  }

  dispose(): void {
    this.latest.abort()
    this.unsubscribeTracks?.()
    this.changed.clear()
  }

  private pose(state: OrbitViewState): OrbitCamera {
    return {
      rotationX: state.rotationX ?? FIT_ROTATION.rotationX,
      rotationOrbit: state.rotationOrbit ?? FIT_ROTATION.rotationOrbit,
      zoom: state.zoom,
      target: this.transform.fromWorld(state.target),
    }
  }

  private levelUnit(levels: readonly Level[], level: number): WorldXYZ {
    const found = levels.find((l) => l.index === level)
    // `factor` is [z, y, x] to match Dims and centroid order.
    const [fz, fy, fx] = found?.factor ?? [1, 1, 1]
    const unit = this.transform.unit
    return [unit[0] * fx, unit[1] * fy, unit[2] * fz]
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
