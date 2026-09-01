import type { CellRow, EditResult, LayerId } from '@cellstudio/api-client'
import type { GraphApi, MaskApi, PixelApi } from '../data/api'
import { GpuBudget } from '../data/gpuBudget'
import type { PlaneKey, VolumeKey } from '../data/keys'
import { PlaneCache } from '../data/planeCache'
import { TSettleWarmer, type BrickShape } from '../data/prefetch'
import { RemapCache } from '../data/trackFrame'
import { TrackSource } from '../data/trackSource'
import { VolumeCache } from '../data/volumeCache'
import { loadXyPyramid, type ZarrStoreLike } from '../data/xySource'
import { MaskEditor } from '../edit/maskEditor'
import type { LineageOverlay } from '../layers/tracks'
import { PerfMonitor } from '../perf'
import { pendingLinkStale, useNav, type ActiveView, type SliceOrientation } from '../state/nav'
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
  /** A committed link: disarm `pendingLink` and revert to the pointer. */
  completeLink?(): void
  /** A stale `pendingLink`: disarm without a POST. */
  cancelLink?(): void
  /** A picked trail edge, for cutting one link (null clears it). */
  selectLink?(link: { parent: number; child: number } | null): void
  /** Patches the label echo after a mask edit commits. */
  setLabelsVersion?(version: number): void
  /** Patches the graph echo after a graph edit commits. */
  setGraphVersion?(version: number): void
}

/** What a graph commit reported changed; empty means "refetch everything". */
export interface GraphAffected {
  cells?: readonly CellRow[]
  tracks?: readonly number[]
}

export interface ViewerSessionOptions {
  api: PixelApi & MaskApi & Partial<GraphApi>
  /** zarrita store over the raw `/store` passthrough; XY renders once this is set. */
  store?: ZarrStoreLike
  perf?: PerfMonitor
  budget?: GpuBudget
  planeCapacityBytes?: number
  volumeCapacityBytes?: number
  brick?: BrickShape
  tSettleMs?: number
  nav?: NavActions
  onEditError?(error: unknown): void
}

const ORIENTATIONS: SliceOrientation[] = ['xy', 'xz', 'yz']

/** A structured server rejection (409 with a reason) reads better than the raw HTTP line. */
const rejectionReason = (error: unknown): unknown => {
  if (error && typeof error === 'object' && 'detail' in error) {
    const detail = (error as { detail: unknown }).detail
    if (typeof detail === 'string' && detail.length > 0) return new Error(detail)
  }
  return error
}

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
  /** `VolumeCache.configure` holds one prefetch context, so labels get their own. */
  readonly labelVolumes: VolumeCache
  readonly tracks: TrackSource
  /** Remapped display copies of label buffers, dropped whole on a graph advance. */
  readonly remaps: RemapCache
  readonly editor: MaskEditor
  readonly readout: CursorReadout
  readonly slices: Record<SliceOrientation, SliceScene>
  readonly volumeScene: VolumeScene
  private readonly warmer: TSettleWarmer
  private readonly store?: ZarrStoreLike
  private readonly changed = new Emitter()
  private readonly unsubscribes: (() => void)[] = []
  private readonly navActions: NavActions
  private readonly graphApi: Partial<GraphApi>
  private readonly onEditError?: (error: unknown) => void
  private nav: NavSnapshot | null = null
  private current: SceneStatus = IDLE_STATUS
  private labelsApplied = 0
  private graphApplied = 0
  private readonly graphListeners = new Set<
    (graphVersion: number, affected: GraphAffected) => void
  >()
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
    this.labelVolumes = new VolumeCache({
      api: opts.api,
      capacity: opts.volumeCapacityBytes,
      perf: this.perf,
    })
    this.tracks = new TrackSource(opts.api)
    this.remaps = new RemapCache()
    this.readout = new CursorReadout({ api: opts.api, tracks: this.tracks })
    const nav = opts.nav ?? {
      select: (cellId: number | null) => useNav.getState().select(cellId),
      jumpToCell: (cell: CellRow, view?: ActiveView) => useNav.getState().jumpToCell(cell, view),
      setLabelsVersion: (version: number) => useNav.getState().setLabelsVersion(version),
      setGraphVersion: (version: number) => useNav.getState().setGraphVersion(version),
      completeLink: () => useNav.getState().completeLink(),
      cancelLink: () => useNav.getState().cancelLink(),
    }
    this.navActions = nav
    this.graphApi = opts.api
    this.onEditError = opts.onEditError
    this.editor = new MaskEditor({
      api: opts.api,
      onCommit: (result) => this.dispatchEdit(result),
      onError: opts.onEditError,
      // The new cell continues under the next stroke, which is what selecting it buys.
      onLabel: (label) => nav.select(label),
    })
    const slice = (orientation: SliceOrientation) =>
      new SliceScene({
        orientation,
        planes: this.planes,
        tracks: this.tracks,
        remaps: this.remaps,
        perf: this.perf,
        budget: this.budget,
        brick: opts.brick,
        api: opts.api,
        editor: this.editor,
        onSelect: (cell) => nav.select(cell.id),
        // A slice-view jump stays in the current view.
        onJumpToCell: (cell) => nav.jumpToCell(cell),
        onSelectLabel: (cellId) => nav.select(cellId),
        onLinkTarget: (cellId) => this.completePendingLink(cellId),
        onSelectLink: (link) => nav.selectLink?.(link),
      })
    this.slices = { xy: slice('xy'), xz: slice('xz'), yz: slice('yz') }
    this.volumeScene = new VolumeScene({
      volumes: this.volumes,
      labelVolumes: this.labelVolumes,
      editor: this.editor,
      tracks: this.tracks,
      remaps: this.remaps,
      perf: this.perf,
      budget: this.budget,
      onSelect: (cell) => nav.select(cell.id),
      onJumpToCell: (cell) => nav.jumpToCell(cell, 'xy'),
      onLinkTarget: (cellId) => this.completePendingLink(cellId),
      onSelectLink: (link) => nav.selectLink?.(link),
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

  /** The project opened with a label store, or an edit this session created one. */
  get labelsPresent(): boolean {
    return this.editor.labelsPresent
  }

  /** Mask writes queued or in flight — the status bar's unsaved count. */
  get pendingWrites(): number {
    return this.editor.pendingWrites
  }

  /**
   * One path for both arrivals of a commit: the mask response and the WS invalidate.
   * Idempotent by version, so whichever lands first does the work and the second returns
   * — response-then-event and event-then-response both produce one fetch.   */
  advanceLabels(
    sessionId: string,
    version: number,
    cells: readonly CellRow[] = [],
    removed: readonly number[] = [],
  ): boolean {
    if (this.lastSession !== null && sessionId !== this.lastSession) return false
    if (version <= this.labelsApplied) return false
    this.labelsApplied = version
    this.navActions.setLabelsVersion?.(version)
    this.planes.invalidate('labels', version)
    this.labelVolumes.invalidate('labels', version)
    this.tracks.patch(cells, removed)
    const nav = this.nav
    if (nav?.project) {
      // The refetch happens now rather than on the next nav write.
      this.update({
        ...nav,
        project: {
          ...nav.project,
          versions: { ...nav.project.versions, labels: version },
        },
      })
    }
    return true
  }

  /**
   * The graph twin of `advanceLabels`, and just as idempotent: whichever of the HTTP edit
   * result, the `graphChanged` event and a reconnect's `Versions` lands first does the
   * work. Voxels are untouched, so no generation bump and no label refetch — the track
   * window aborts and re-versions, the remapped display copies drop, and the active scene
   * reruns now.   */
  advanceGraph(sessionId: string, graphVersion: number, affected: GraphAffected = {}): boolean {
    if (this.lastSession !== null && sessionId !== this.lastSession) return false
    if (graphVersion <= this.graphApplied) return false
    this.graphApplied = graphVersion
    this.navActions.setGraphVersion?.(graphVersion)
    this.tracks.setGraphVersion(graphVersion)
    this.remaps.clear()
    // Committed rows draw immediately; the window stays stale until the versioned refetch.
    if (affected.cells?.length) this.tracks.patch(affected.cells, [])
    for (const cb of this.graphListeners) cb(graphVersion, affected)
    const nav = this.nav
    if (nav?.project) {
      this.update({
        ...nav,
        project: {
          ...nav.project,
          versions: { ...nav.project.versions, graph: graphVersion },
        },
      })
    }
    return true
  }

  /** The lineage refetch hook: fired after every applied `advanceGraph`. */
  onGraphAdvance(cb: (graphVersion: number, affected: GraphAffected) => void): () => void {
    this.graphListeners.add(cb)
    return () => this.graphListeners.delete(cb)
  }

  /**
   * The selected lineage flowing into both scenes An overlay fetched under an
   * older graph version than the one already applied never lands.
   */
  setLineage(overlay: LineageOverlay | null): void {
    if (overlay && overlay.graphVersion < this.graphApplied) return
    for (const o of ORIENTATIONS) this.slices[o].setLineage(overlay)
    this.volumeScene.setLineage(overlay)
  }

  /**
   * An armed link's completing click The pendingLink is validated against
   * the live session and graph version; a same-or-earlier-frame target and a server
   * rejection surface their reason without disarming, success disarms and reverts the
   * tool through `NavActions.completeLink`.
   */
  completePendingLink(childId: number): void {
    const nav = this.nav
    const pending = nav?.pendingLink
    const project = nav?.project
    if (!pending || !project) return
    if (pendingLinkStale(pending, project)) {
      this.navActions.cancelLink?.()
      this.onEditError?.(new Error('Link cancelled: the graph changed while the link was armed'))
      return
    }
    if (childId === pending.parentId) {
      this.onEditError?.(new Error('Link rejected: a cell cannot link to itself'))
      return
    }
    const parent = this.tracks.cell(pending.parentId)
    const child = this.tracks.cell(childId)
    if (parent && child && child.t <= parent.t) {
      this.onEditError?.(new Error('Link rejected: the target must be at a later frame'))
      return
    }
    const link = this.graphApi.link
    if (!link) return
    void link
      .call(this.graphApi, { parentId: pending.parentId, childId })
      .then((result) => {
        this.navActions.completeLink?.()
        this.dispatchEdit(result)
      })
      .catch((error) => this.onEditError?.(rejectionReason(error)))
  }

  /** Unlink the selected track. posted immediately, undoable through history. */
  unlinkCell(cellId: number): void {
    const unlink = this.graphApi.unlink
    if (!unlink) return
    void unlink
      .call(this.graphApi, { cellId })
      .then((result) => this.dispatchEdit(result))
      .catch((error) => this.onEditError?.(rejectionReason(error)))
  }

  /** Cuts one link — the selected edge — instead of deleting the whole track. */
  cutLink(parentId: number, childId: number): void {
    const cut = this.graphApi.cut
    if (!cut) return
    void cut
      .call(this.graphApi, { parentId, childId })
      .then((result) => this.dispatchEdit(result))
      .catch((error) => this.onEditError?.(rejectionReason(error)))
  }

  /** Every HTTP edit result routes here by domain; mask results with a graph bump hit both. */
  private dispatchEdit(result: EditResult): void {
    if (result.domain === 'graph') {
      this.advanceGraph(result.sessionId, result.graphVersion, {
        cells: result.affectedCells,
        tracks: result.affectedTracks,
      })
      return
    }
    this.advanceLabels(result.sessionId, result.version, result.cells, result.removed)
    if (result.graphVersion !== undefined) {
      this.advanceGraph(result.sessionId, result.graphVersion, {
        tracks: result.affectedTracks ?? [],
      })
    }
  }

  /** Drives the active view; inactive views warm on t-settle rather than per step. */
  update(nav: NavSnapshot): void {
    if (!nav.project) return
    this.nav = nav
    this.openGate(nav)
    this.editor.configure({
      dims: [nav.project.dims.z, nav.project.dims.y, nav.project.dims.x],
      scale: nav.project.scale,
      storePresent: nav.project.hasLabels,
    })
    // A lease is in hand before the first gesture, and replenished under it.
    if (nav.tool === 'brush' || nav.tool === 'eraser') this.editor.ensureLease()
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
    this.labelVolumes.invalidate(layer, version)
    if (layer === 'image') this.pyramidVersion = null
  }

  dispose(): void {
    this.warmer.cancel()
    for (const unsubscribe of this.unsubscribes) unsubscribe()
    this.unsubscribes.length = 0
    for (const o of ORIENTATIONS) this.slices[o].dispose()
    this.volumeScene.dispose()
    this.readout.dispose()
    this.tracks.dispose()
    this.editor.dispose()
    this.planes.clear()
    this.volumes.clear()
    this.labelVolumes.clear()
    this.remaps.clear()
    this.graphListeners.clear()
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
    this.editor.reset()
    for (const o of ORIENTATIONS) this.slices[o].reset()
    this.volumeScene.reset()
    this.planes.clear()
    this.volumes.clear()
    this.labelVolumes.clear()
    this.remaps.clear()
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
      this.labelsApplied = nav.project?.versions.labels ?? 0
      this.graphApplied = nav.project?.versions.graph ?? 0
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
