import type {
  Dtype,
  EditResult,
  MaskMode,
  PhysicalScale,
  PlaneBuffer,
  StampAxis,
  StrokeBody,
  VolumeBuffer,
} from '@cellstudio/api-client'
import type { MaskApi } from '../data/api'
import type { PixelZYX } from '../data/world'
import { Emitter } from '../scenes/types'
import {
  AXIS_SLOT,
  downsample,
  stampRadii,
  stampVoxels,
  unionVoxelSets,
  type StampPlane,
  type VoxelSet,
} from './stamp'

/** Ids taken per reservation, and the remainder that triggers the next one. */
export const LEASE_SIZE = 64
export const LEASE_LOW_WATER = 8
/** Past this the stroke flushes and a second one continues it. */
export const MAX_STROKE_STAMPS = 4096
/** A new stamp once the pointer has moved this fraction of a radius. */
const COALESCE = 1 / 3

export interface MaskEditorOptions {
  api: MaskApi
  /** Every committed result; the session dispatches by domain to advanceLabels/advanceGraph. */
  onCommit(result: EditResult): void
  onError?(error: unknown): void
  /** A new id taken for a stroke; the session selects it so the next stroke continues it. */
  onLabel?(label: number): void
  leaseSize?: number
}

export interface MaskEditorConfig {
  /** Level-0 dataset dims, `[z, y, x]`. */
  dims: PixelZYX
  scale: PhysicalScale | null
  /** The project opened with a label store. */
  storePresent: boolean
}

export interface StrokeStart {
  t: number
  tool: 'brush' | 'eraser'
  radius: number
  /** The pinned slice, or null for the 3D orb. */
  plane: StampPlane | null
  centre: PixelZYX
  /** `nav.selection.cellId`, when that cell exists on this frame. */
  selection: number | null
}

/** One label plane as the view draws it: level geometry plus the base's label version. */
export interface LabelPlaneView {
  axis: StampAxis
  /** Level-0 index along `axis`. */
  index: number
  t: number
  /** Level factor, `[z, y, x]`. */
  factor: PixelZYX
  /** Level pixel size of the plane, `[height, width]`. */
  shape: [number, number]
  level: number
  /** Operations acknowledged at or below this are already in `base`. */
  version: number
}

export interface LabelVolumeView {
  t: number
  /** Level factor, `[z, y, x]`. */
  factor: PixelZYX
  /** Level voxel size, `[z, y, x]`. */
  dims: PixelZYX
  level: number
  version: number
}

/** One level-0 operation, and the version that made it authoritative. */
interface PendingOp {
  t: number
  label: number
  mode: MaskMode
  only: number | null
  plane: StampPlane | null
  radius: number
  stamps: PixelZYX[]
  voxels: VoxelSet
  ackVersion: number | null
}

interface Task {
  /** The echo this write carries, or null for a delete, undo or redo. */
  op: PendingOp | null
  run(): Promise<EditResult>
}

/** A writable u32 copy of the authoritative buffer, or zeros when there is none. */
function u32Copy(
  base: { dtype: Dtype; data: ArrayBuffer } | null,
  length: number,
): { data: ArrayBuffer; out: Uint32Array } {
  const data = new ArrayBuffer(length * 4)
  const out = new Uint32Array(data)
  if (!base) return { data, out }
  const view =
    base.dtype === 'u8'
      ? new Uint8Array(base.data)
      : base.dtype === 'u16'
        ? new Uint16Array(base.data)
        : new Uint32Array(base.data)
  out.set(view.subarray(0, Math.min(length, view.length)))
  return { data, out }
}

/** `[row, col0, col1]` spans a level-0 set covers on one plane of a display level. */
function planeSpans(voxels: VoxelSet, view: LabelPlaneView): [number, number, number][] {
  const factor = view.factor[AXIS_SLOT[view.axis]]
  // A coarse level point-samples level 0., so a slice between samples has none.
  if (factor <= 0 || view.index % factor !== 0) return []
  const index = view.index / factor
  const out: [number, number, number][] = []
  for (const run of downsample(voxels, view.factor).runs) {
    if (view.axis === 'z') {
      if (run.z === index) out.push([run.y, run.x0, run.x1])
    } else if (view.axis === 'y') {
      if (run.y === index) out.push([run.z, run.x0, run.x1])
    } else if (run.x0 <= index && index <= run.x1) {
      out.push([run.z, run.y, run.y])
    }
  }
  return out
}

/**
 * The renderer's half of the edit pipeline: an immutable authoritative buffer plus an
 * ordered log of level-0 operations that have not been acknowledged yet. * What the layers draw is base + pending, replayed — so a refreshed base replays what is
 * still pending, and a failed write removes exactly one operation.
 */
export class MaskEditor {
  private readonly api: MaskApi
  private readonly commit: (result: EditResult) => void
  private readonly fail?: (error: unknown) => void
  private readonly announce?: (label: number) => void
  private readonly leaseSize: number
  private readonly changed = new Emitter()
  private ops: PendingOp[] = []
  private active: PendingOp | null = null
  private queue: Task[] = []
  private running: Task | null = null
  private lease: { next: number; end: number } | null = null
  private leasing = false
  private storePresent = false
  private committedEdit = false
  private dims: PixelZYX = [0, 0, 0]
  private scale: PhysicalScale | null = null
  private disposed = false

  constructor(opts: MaskEditorOptions) {
    this.api = opts.api
    this.commit = opts.onCommit
    this.fail = opts.onError
    this.announce = opts.onLabel
    this.leaseSize = opts.leaseSize ?? LEASE_SIZE
  }

  onChange(cb: () => void): () => void {
    return this.changed.on(cb)
  }

  /** Writes queued or in flight — the status bar's unsaved count. */
  get pendingWrites(): number {
    return this.queue.length + (this.running ? 1 : 0)
  }

  /** Operations the drawn view still replays; a test's window on the log. */
  get pendingOps(): number {
    return this.ops.length
  }

  /** The store exists: it opened with one, or an edit this session created it. */
  get labelsPresent(): boolean {
    return this.storePresent || this.committedEdit
  }

  configure(cfg: MaskEditorConfig): void {
    this.dims = cfg.dims
    this.scale = cfg.scale
    this.storePresent = cfg.storePresent
  }

  /** Called when a paint tool becomes active, and again below the low-water mark. */
  ensureLease(): void {
    const remaining = this.lease ? this.lease.end - this.lease.next : 0
    if (this.disposed || this.leasing || remaining > LEASE_LOW_WATER) return
    this.leasing = true
    void this.api
      .reserveLabels(this.leaseSize)
      .then((lease) => {
        if (this.disposed) return
        this.lease = { next: lease.first, end: lease.first + lease.count }
      })
      .catch((e) => this.report(e))
      .finally(() => {
        this.leasing = false
      })
  }

  /**
   * Starts a stroke, echoing its first stamp. False when it cannot start — a new id with
   * no lease in hand would paint what the server is certain to reject.   */
  begin(start: StrokeStart): boolean {
    this.cancel()
    const erase = start.tool === 'eraser'
    const label = erase ? (start.selection ?? 0) : (start.selection ?? this.takeLease())
    if (label === null) {
      this.report(new Error('No label ids reserved — the stroke was not started'))
      return false
    }
    const op: PendingOp = {
      t: start.t,
      label,
      mode: erase ? 'erase' : 'paint',
      only: erase ? start.selection : null,
      plane: start.plane,
      radius: start.radius,
      stamps: [],
      voxels: { runs: [] },
      ackVersion: null,
    }
    this.ops.push(op)
    this.active = op
    this.stamp(op, start.centre)
    if (!erase && start.selection === null) this.announce?.(label)
    this.changed.emit()
    return true
  }

  /** One stamp per `r/3` of pointer travel, measured in the stamp's own ellipsoid metric. */
  move(centre: PixelZYX): void {
    const op = this.active
    if (!op) return
    const last = op.stamps[op.stamps.length - 1]
    if (last && this.travel(last, centre, op.radius) < COALESCE) return
    this.stamp(op, centre)
    this.changed.emit()
    if (op.stamps.length >= MAX_STROKE_STAMPS) {
      const next: StrokeStart = {        t: op.t,
        tool: op.mode === 'erase' ? 'eraser' : 'brush',
        radius: op.radius,
        plane: op.plane,
        centre,
        selection: op.mode === 'erase' ? op.only : op.label,
      }
      this.end()
      this.begin(next)
    }
  }



  end(): void {
    const op = this.active
    if (!op) return
    this.active = null
    this.enqueue({ op, run: () => this.api.stroke(this.body(op)) })
  }

  /** Discards the live stroke: it leaves the log and nothing is written. */
  cancel(): void {
    const op = this.active
    if (!op) return
    this.active = null
    this.drop(op)
    this.changed.emit()
  }

  deleteMask(t: number, label: number): void {
    this.enqueue({ op: null, run: () => this.api.deleteMask({ t, label }) })
  }

  undo(): void {
    this.enqueue({ op: null, run: () => this.api.undo() })
  }

  redo(): void {
    this.enqueue({ op: null, run: () => this.api.redo() })
  }

  /** Drops the operations `version` made authoritative; the base being drawn holds them. */
  reconcile(version: number): void {
    this.ops = this.ops.filter((op) => op.ackVersion === null || op.ackVersion > version)
  }

  /** The label plane as drawn: the authoritative base with the pending log replayed. */
  planeBuffer(view: LabelPlaneView, base: PlaneBuffer | null): PlaneBuffer | null {
    this.reconcile(view.version)
    const [height, width] = view.shape
    const work: [PendingOp, [number, number, number][]][] = []
    for (const op of this.replayed(view.t, view.version)) {
      const spans = planeSpans(op.voxels, view)
      if (spans.length > 0) work.push([op, spans])
    }
    if (work.length === 0) return base
    const { data, out } = u32Copy(base, height * width)
    for (const [op, spans] of work) {
      for (const [row, col0, col1] of spans) {
        if (row < 0 || row >= height) continue
        const start = row * width + Math.max(col0, 0)
        const end = row * width + Math.min(col1, width - 1)
        this.write(out, start, end, op)
      }
    }
    return {
      shape: [height, width],
      channels: 1,
      dtype: 'u32',
      level: base?.level ?? view.level,
      data,
    }
  }

  /** The label volume as drawn, on the same base-plus-pending rule. */
  volumeBuffer(view: LabelVolumeView, base: VolumeBuffer | null): VolumeBuffer | null {
    this.reconcile(view.version)
    const [depth, height, width] = view.dims
    const work: [PendingOp, VoxelSet][] = []
    for (const op of this.replayed(view.t, view.version)) {
      const coarse = downsample(op.voxels, view.factor)
      if (coarse.runs.length > 0) work.push([op, coarse])
    }
    if (work.length === 0) return base
    const { data, out } = u32Copy(base, depth * height * width)
    for (const [op, coarse] of work) {
      for (const run of coarse.runs) {
        if (run.z < 0 || run.z >= depth || run.y < 0 || run.y >= height) continue
        const row = (run.z * height + run.y) * width
        this.write(out, row + Math.max(run.x0, 0), row + Math.min(run.x1, width - 1), op)
      }
    }
    return { shape: [depth, height, width], dtype: 'u32', level: view.level, data }
  }

  /** A new backend session shares no ids, no log and no store state with the old one. */
  reset(): void {
    this.active = null
    this.ops = []
    this.queue = []
    this.lease = null
    this.committedEdit = false
    this.changed.emit()
  }

  dispose(): void {
    this.disposed = true
    this.active = null
    this.ops = []
    this.queue = []
    this.changed.clear()
  }

  private replayed(t: number, version: number): PendingOp[] {
    return this.ops.filter(
      (op) => op.t === t && (op.ackVersion === null || op.ackVersion > version),
    )
  }

  private write(out: Uint32Array, start: number, end: number, op: PendingOp): void {
    if (op.mode === 'paint') {
      out.fill(op.label, start, end + 1)
      return
    }
    // A scoped eraser clears only the cell it is protecting the neighbour from.
    for (let i = start; i <= end; i++) {
      if (op.only === null || out[i] === op.only) out[i] = 0
    }
  }

  private stamp(op: PendingOp, centre: PixelZYX): void {
    const pinned = this.pin(centre, op.plane)
    op.stamps.push(pinned)
    const set = stampVoxels(pinned, op.radius, this.scale, op.plane, this.dims)
    op.voxels = op.stamps.length === 1 ? set : unionVoxelSets([op.voxels, set])
  }

  /** The stamp centre sits at the middle of the pinned voxel, so the server pins the same one. */
  private pin(centre: PixelZYX, plane: StampPlane | null): PixelZYX {
    if (!plane) return centre
    const out: [number, number, number] = [centre[0], centre[1], centre[2]]
    out[AXIS_SLOT[plane.axis]] = plane.index + 0.5
    return out
  }

  private travel(from: PixelZYX, to: PixelZYX, radius: number): number {
    const rad = stampRadii(radius, this.scale)
    let sum = 0
    for (let i = 0; i < 3; i++) {
      const d = ((to[i] ?? 0) - (from[i] ?? 0)) / (rad[i] as number)
      sum += d * d
    }
    return Math.sqrt(sum)
  }

  private body(op: PendingOp): StrokeBody {
    return {
      t: op.t,
      label: op.label,
      mode: op.mode,
      radius: op.radius,
      plane: op.plane?.axis ?? null,
      stamps: op.stamps.map((s) => [s[0], s[1], s[2]] as [number, number, number]),
      only: op.only,
    }
  }

  private takeLease(): number | null {
    const lease = this.lease
    if (!lease || lease.next >= lease.end) {
      this.ensureLease()
      return null
    }
    const id = lease.next
    lease.next += 1
    this.ensureLease()
    return id
  }

  /** One write in flight, the rest in order behind it, so two strokes cannot interleave. */
  private enqueue(task: Task): void {
    this.queue.push(task)
    this.changed.emit()
    this.pump()
  }

  private pump(): void {
    if (this.running || this.disposed) return
    const task = this.queue.shift()
    if (!task) return
    this.running = task
    void task
      .run()
      .then((result) => this.settle(task, result))
      .catch((e) => {
        // Only the failed operation leaves the log; the base was never patched, so the
        // rest of the pending log still draws over it.
        if (task.op) this.drop(task.op)
        this.report(e)
      })
      .finally(() => {
        this.running = null
        this.changed.emit()
        this.pump()
      })
  }

  private settle(task: Task, result: EditResult): void {
    if (this.disposed) return
    // a graph-domain undo/redo has no labels to reconcile; onCommit routes it to advanceGraph
    if (result.domain === 'graph') {
      this.commit(result)
      return
    }
    if (task.op) task.op.ackVersion = result.version
    if (result.hasLabels) this.committedEdit = true
    this.commit(result)
  }

  private drop(op: PendingOp): void {
    this.ops = this.ops.filter((o) => o !== op)
  }

  private report(error: unknown): void {
    if (this.disposed) return
    this.fail?.(error)
  }
}
