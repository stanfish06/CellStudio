import type { CellRow, LayerId } from '@cellstudio/api-client'
import type { PixelApi } from '../data/api'
import { GpuBudget } from '../data/gpuBudget'
import type { PlaneKey, VolumeKey } from '../data/keys'
import { PlaneCache } from '../data/planeCache'
import { TSettleWarmer, type BrickShape } from '../data/prefetch'
import { TrackSource } from '../data/trackSource'
import { VolumeCache } from '../data/volumeCache'
import { loadXyPyramid, type ZarrStoreLike } from '../data/xySource'
import { PerfMonitor } from '../perf'
import { useNav, type ActiveView, type SliceOrientation } from '../state/nav'
import { CursorReadout } from './cursorReadout'
import { SliceScene } from './sliceScene'
import { VolumeScene } from './volumeScene'
import {
  Emitter,
  levelForZoom,
  sameStatus,
  visibleChannels,
  type NavSnapshot,
  type SceneStatus,
} from './types'

export interface NavActions {
  select(cellId: number | null): void
  jumpToCell(cell: CellRow, view?: ActiveView): void
}

export interface ViewerSessionOptions {
  api: PixelApi
  /** zarrita store over the raw `/store` passthrough; XY renders once this is set. */
  store?: ZarrStoreLike
  perf?: PerfMonitor
  budget?: GpuBudget
  planeCapacityBytes?: number
  volumeCapacityBytes?: number
  brick?: BrickShape
  tSettleMs?: number
  nav?: NavActions
}

const ORIENTATIONS: SliceOrientation[] = ['xy', 'xz', 'yz']

const IDLE_STATUS: SceneStatus = { display: { level: 0, zoom: 0 }, awaitingFrame: false }

/**
 * Everything the pixel plane needs for one backend session. It dies with the `ApiClient`
 * that built it, which is what keeps stale-session data out of the caches.
 */
export class ViewerSession {
  readonly perf: PerfMonitor
  readonly budget: GpuBudget
  readonly planes: PlaneCache
  readonly volumes: VolumeCache
  readonly tracks: TrackSource
  readonly readout: CursorReadout
  readonly slices: Record<SliceOrientation, SliceScene>
  readonly volumeScene: VolumeScene
  private readonly warmer: TSettleWarmer
  private readonly store?: ZarrStoreLike
  private readonly changed = new Emitter()
  private readonly unsubscribes: (() => void)[] = []
  private nav: NavSnapshot | null = null
  private current: SceneStatus = IDLE_STATUS
  private lastT: number | null = null
  private lastView: ActiveView | null = null
  private lastSession: string | null = null
  private pendingGate: string | null = null
  private pyramidVersion: number | null = null

  constructor(opts: ViewerSessionOptions) {
    this.perf = opts.perf ?? new PerfMonitor()
    this.budget = opts.budget ?? new GpuBudget()
    this.store = opts.store
    this.planes = new PlaneCache({
      api: opts.api,
      capacity: opts.planeCapacityBytes,
      brick: opts.brick,
      perf: this.perf,
    })
    this.volumes = new VolumeCache({
      api: opts.api,
      capacity: opts.volumeCapacityBytes,
      perf: this.perf,
    })
    this.tracks = new TrackSource(opts.api)
    this.readout = new CursorReadout({ api: opts.api, tracks: this.tracks })
    const nav = opts.nav ?? {
      select: (cellId: number | null) => useNav.getState().select(cellId),
      jumpToCell: (cell: CellRow, view?: ActiveView) => useNav.getState().jumpToCell(cell, view),
    }
    const slice = (orientation: SliceOrientation) =>
      new SliceScene({
        orientation,
        planes: this.planes,
        tracks: this.tracks,
        perf: this.perf,
        budget: this.budget,
        brick: opts.brick,
        onSelect: (cell) => nav.select(cell.id),
        // A slice-view jump stays in the current view.
        onJumpToCell: (cell) => nav.jumpToCell(cell),
      })
    this.slices = { xy: slice('xy'), xz: slice('xz'), yz: slice('yz') }
    this.volumeScene = new VolumeScene({
      volumes: this.volumes,
      tracks: this.tracks,
      perf: this.perf,
      budget: this.budget,
      onSelect: (cell) => nav.select(cell.id),
      onJumpToCell: (cell) => nav.jumpToCell(cell, 'xy'),
    })
    this.warmer = new TSettleWarmer(
      {
        plane: (key) => this.planes.warm(key),
        volume: (key) => this.volumes.warm(key),
      },
      opts.tSettleMs ?? 150,
    )
    for (const o of ORIENTATIONS) {
      this.unsubscribes.push(this.slices[o].onChange(() => this.sceneChanged()))
    }
    this.unsubscribes.push(this.volumeScene.onChange(() => this.sceneChanged()))
    this.unsubscribes.push(this.readout.onChange(() => this.changed.emit()))
  }

  /** Backend session these scenes and caches belong to; null until the first update. */
  get sessionId(): string | null {
    return this.lastSession
  }

  /**
   * What the active scene is presenting, for the ui's level/zoom chip and playback gate.
   * The object identity only changes when a value does, so it is safe as a store snapshot.
   */
  get status(): SceneStatus {
    return this.current
  }

  /** Fires on every scene, overlay and readout change — the ui's redraw signal. */
  onChange(cb: () => void): () => void {
    return this.changed.on(cb)
  }

  scene(view: ActiveView): SliceScene | VolumeScene {
    return view === '3d' ? this.volumeScene : this.slices[view]
  }

  /** Drives the active view; inactive views warm on t-settle rather than per step. */
  update(nav: NavSnapshot): void {
    if (!nav.project) return
    this.nav = nav
    this.openGate(nav)
    void this.ensurePyramid(nav)
    this.scene(nav.activeView).update(nav)
    if (this.lastT !== nav.t) {
      this.lastT = nav.t
      this.warmer.schedule(this.warmPlan(nav))
    }
    this.refreshStatus()
  }

  /**
   * Call after the frame has painted. Closes the active scene's interaction and, once the
   * destination view actually has data, the open-project or view-switch gate.
   */
  markPresented(nav: NavSnapshot): void {
    const scene = this.scene(nav.activeView)
    scene.markPresented()
    if (this.pendingGate && !scene.status().awaitingFrame) {
      this.perf.presented(this.pendingGate)
      this.pendingGate = null
    }
    this.refreshStatus()
  }

  /** Version bumps: drop what the bump invalidated and let the next update refetch. */
  invalidate(layer: LayerId, version: number): void {
    this.planes.invalidate(layer, version)
    this.volumes.invalidate(layer, version)
    if (layer === 'image') this.pyramidVersion = null
  }

  graphChanged(): void {
    this.tracks.invalidate()
  }

  dispose(): void {
    this.warmer.cancel()
    for (const unsubscribe of this.unsubscribes) unsubscribe()
    this.unsubscribes.length = 0
    for (const o of ORIENTATIONS) this.slices[o].dispose()
    this.volumeScene.dispose()
    this.readout.dispose()
    this.tracks.dispose()
    this.planes.clear()
    this.volumes.clear()
    this.changed.clear()
    this.nav = null
    this.current = IDLE_STATUS
  }

  private sceneChanged(): void {
    this.refreshStatus()
    this.changed.emit()
  }

  private refreshStatus(): void {
    const next = this.nav ? this.scene(this.nav.activeView).status() : IDLE_STATUS
    if (!sameStatus(next, this.current)) this.current = next
  }

  /**
   * A new backend session shares no bytes with the old one: the renderer's keys carry no
   * session id, so they have to be dropped rather than matched.
   */
  private reset(): void {
    this.warmer.cancel()
    this.readout.clear()
    this.tracks.reset()
    for (const o of ORIENTATIONS) this.slices[o].reset()
    this.volumeScene.reset()
    this.planes.clear()
    this.volumes.clear()
    this.pyramidVersion = null
    this.lastT = null
    this.current = IDLE_STATUS
  }

  /** Opens the interaction a switch or a project open is measured against. */
  private openGate(nav: NavSnapshot): void {
    const sessionId = nav.project?.sessionId ?? null
    if (sessionId !== this.lastSession) {
      if (this.lastSession !== null) this.reset()
      this.lastSession = sessionId
      this.lastView = nav.activeView
      this.pendingGate = `open:${sessionId}:${nav.activeView}`
      this.perf.begin(nav.activeView === '3d' ? 'first-volume' : 'first-pixels', this.pendingGate)
      return
    }
    if (nav.activeView !== this.lastView) {
      this.lastView = nav.activeView
      this.pendingGate = `switch:${nav.activeView}:${nav.generation}`
      this.perf.begin('view-switch', this.pendingGate)
    }
  }

  private async ensurePyramid(nav: NavSnapshot): Promise<void> {
    const version = nav.project?.versions.image
    if (!this.store || version === undefined || this.pyramidVersion === version) return
    this.pyramidVersion = version
    try {
      const pyramid = await loadXyPyramid(this.store, version)
      this.slices.xy.setPyramid(pyramid)
    } catch {
      this.pyramidVersion = null
    }
  }

  private warmPlan(nav: NavSnapshot): { planes: PlaneKey[]; volumes: VolumeKey[] } {
    const project = nav.project
    if (!project) return { planes: [], volumes: [] }
    const channels = visibleChannels(nav.channels).map((c) => c.index)
    const planes: PlaneKey[] = []
    for (const orientation of ['xz', 'yz'] as const) {
      if (nav.activeView === orientation || channels.length === 0) continue
      planes.push({
        layer: 'image',
        axis: orientation,
        level: levelForZoom(nav.slices[orientation].camera.zoom, project.levels),
        t: nav.t,
        c: channels,
        index: nav.slices[orientation].index,
        version: project.versions.image,
      })
    }
    const volumes: VolumeKey[] =
      nav.activeView === '3d'
        ? []
        : channels.map((c) => ({
            layer: 'image' as const,
            level: this.budget.planVolume(project.levels, channels.length, project.dtype).level,
            t: nav.t,
            c,
            version: project.versions.image,
          }))
    return { planes, volumes }
  }
}
