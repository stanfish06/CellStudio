import { z } from 'zod'

/** Ids are u32 end-to-end so they stay exactly representable as JS numbers. */
const u32 = z.number().int().min(0).max(4294967295)

export const LayerId = z.enum(['image', 'labels'])
export type LayerId = z.infer<typeof LayerId>

export const Dtype = z.enum(['u8', 'u16', 'u32'])
export type Dtype = z.infer<typeof Dtype>

export const Dims = z.object({
  t: z.number().int().nonnegative(),
  c: z.number().int().nonnegative(),
  z: z.number().int().nonnegative(),
  y: z.number().int().nonnegative(),
  x: z.number().int().nonnegative(),
})
export type Dims = z.infer<typeof Dims>

export const PhysicalScale = z.object({ z: z.number(), y: z.number(), x: z.number() })
export type PhysicalScale = z.infer<typeof PhysicalScale>

export const Level = z.object({
  index: z.number().int().nonnegative(),
  dims: Dims,
  chunks: Dims,
  /** Downsample factor per spatial axis relative to level 0, as stored. */
  factor: z.tuple([z.number(), z.number(), z.number()]),
})
export type Level = z.infer<typeof Level>

export const ChannelMeta = z.object({
  name: z.string(),
  /** Hex RGB without '#', from omero metadata when present. */
  color: z.string().nullable(),
  window: z.tuple([z.number(), z.number()]).nullable(),
})
export type ChannelMeta = z.infer<typeof ChannelMeta>

export const Versions = z.object({
  sessionId: z.string(),
  image: z.number().int().nonnegative(),
  labels: z.number().int().nonnegative(),
  graph: z.number().int().nonnegative(),
  settings: z.number().int().nonnegative(),
})
export type Versions = z.infer<typeof Versions>

/** Per-orientation read amplification; hostile when any primary view exceeds 4x. */
export const LayoutAdvisory = z.object({
  hostile: z.boolean(),
  amplification: z.object({ xy: z.number(), xz: z.number(), yz: z.number() }),
  affectedViews: z.array(z.enum(['xy', 'xz', 'yz'])),
})
export type LayoutAdvisory = z.infer<typeof LayoutAdvisory>

export const ProjectInfo = z.object({
  sessionId: z.string(),
  sourcePath: z.string(),
  projectPath: z.string(),
  dims: Dims,
  dtype: Dtype,
  scale: PhysicalScale.nullable(),
  levels: z.array(Level),
  channels: z.array(ChannelMeta),
  versions: Versions,
  layout: LayoutAdvisory,
  hasLabels: z.boolean(),
  /**
   * Any `links` row exists — a snapshot at open/refetch, refreshed with the next
   * ProjectInfo fetch. The server always sends it; optional so existing constructors
   * stay valid.
   */
  hasGraph: z.boolean().optional(),
})
export type ProjectInfo = z.infer<typeof ProjectInfo>

export const CellState = z.enum(['normal', 'dividing', 'death'])
export type CellState = z.infer<typeof CellState>

export const CellRow = z.object({
  id: u32,
  t: z.number().int().nonnegative(),
  /** Centroid in pixel units, [z, y, x] — same order as the tracking JSON. */
  centroid: z.tuple([z.number(), z.number(), z.number()]).nullable(),
  area: z.number().int().nullable(),
  confidence: z.number().nullable(),
  state: CellState.nullable(),
  trackId: u32.nullable(),
  /** The parent cell's id, so a trail can walk from a daughter into its parent track. */
  parentId: u32.nullable(),
  reviewed: z.boolean(),
})
export type CellRow = z.infer<typeof CellRow>

export const LinkRow = z.object({
  parent: u32,
  child: u32,
  confidence: z.number().nullable(),
  reviewed: z.boolean(),
})
export type LinkRow = z.infer<typeof LinkRow>

export const LineageTree = z.object({
  rootCellId: u32,
  /** The cell the tree was requested for; the client re-centres highlights on it. */
  focusCellId: u32,
  cells: z.array(CellRow),
  links: z.array(LinkRow),
})
export type LineageTree = z.infer<typeof LineageTree>

export const Histogram = z.object({
  /** Bin counts over [min, max]; the popover draws these directly. */
  counts: z.array(z.number()),
  min: z.number(),
  max: z.number(),
  /** True when computed from a sampled coarse level rather than full data. */
  sampled: z.boolean(),
})
export type Histogram = z.infer<typeof Histogram>

export const JobKind = z.enum([
  'rechunk',
  'proxy',
  'inventory',
  'import-tracks',
  'import-labels',
  'export',
])
export type JobKind = z.infer<typeof JobKind>

export const JobState = z.object({
  id: z.string(),
  kind: JobKind,
  progress: z.number().min(0).max(1),
  status: z.enum(['running', 'done', 'failed', 'cancelled']),
  message: z.string().nullable(),
})
export type JobState = z.infer<typeof JobState>

export const ServerEvent = z.discriminatedUnion('type', [
  z.object({ type: z.literal('versions'), versions: Versions }),
  z.object({ type: z.literal('job'), job: JobState }),
  z.object({
    type: z.literal('invalidate'),
    sessionId: z.string(),
    layer: LayerId,
    chunks: z.array(z.string()),
    version: z.number().int().nonnegative(),
  }),
  z.object({
    type: z.literal('graphChanged'),
    /** Graph versions are not comparable across projects, so the event names its session. */
    sessionId: z.string(),
    graphVersion: z.number().int().nonnegative(),
    tracks: z.array(u32),
  }),
])
export type ServerEvent = z.infer<typeof ServerEvent>

export const EditDomain = z.enum(['mask', 'graph'])
export type EditDomain = z.infer<typeof EditDomain>

export const EditEntry = z.object({
  seq: z.number().int(),
  ts: z.string(),
  domain: EditDomain,
  scope: z.string().nullable(),
  undone: z.boolean(),
  /** False once the entry's chunk snapshots have been pruned past the retained window. */
  undoable: z.boolean(),
})
export type EditEntry = z.infer<typeof EditEntry>

/** Mask edits. `plane` pins one axis for a slice-view disk; null is a 3D orb. */
export const MaskMode = z.enum(['paint', 'erase'])
export type MaskMode = z.infer<typeof MaskMode>

export const StampAxis = z.enum(['z', 'y', 'x'])
export type StampAxis = z.infer<typeof StampAxis>

export const StrokeBody = z.object({
  t: z.number().int().nonnegative(),
  label: u32,
  mode: MaskMode,
  /** Level-0 pixels along x; other axes scale by voxel size. */
  radius: z.number().positive(),
  plane: StampAxis.nullable(),
  /** Stamp centres in level-0 pixels, [z, y, x], fractional. */
  stamps: z.array(z.tuple([z.number(), z.number(), z.number()])).min(1),
  /** Eraser scope: clear only this label, or any label when null. */
  only: u32.nullable(),
})
export type StrokeBody = z.infer<typeof StrokeBody>

export const DeleteMaskBody = z.object({
  t: z.number().int().nonnegative(),
  label: u32,
})
export type DeleteMaskBody = z.infer<typeof DeleteMaskBody>

export const LabelLease = z.object({ first: u32, count: z.number().int().positive() })
export type LabelLease = z.infer<typeof LabelLease>

export const MaskEditResult = z.object({
  /** The server always sends 'mask'; optional so pre-union constructors stay valid. */
  domain: z.literal('mask').optional(),
  seq: z.number().int().nonnegative(),
  version: z.number().int().nonnegative(),
  sessionId: z.string(),
  hasLabels: z.boolean(),
  /** Cells whose voxels changed, with recomputed centroid and area. */
  cells: z.array(CellRow),
  /** Cells that no longer exist on that frame — erased to nothing, or deleted. */
  removed: z.array(u32),
  chunks: z.array(z.string()),
  /** `version.graph` after the commit, when the mask edit removed cells or links. */
  graphVersion: z.number().int().nonnegative().optional(),
  affectedTracks: z.array(u32).optional(),
})
export type MaskEditResult = z.infer<typeof MaskEditResult>

/** What one committed graph edit (link, unlink, or their undo/redo) tells the renderer. */
export const GraphEditResult = z.object({
  domain: z.literal('graph'),
  sessionId: z.string(),
  seq: z.number().int().nonnegative(),
  graphVersion: z.number().int().nonnegative(),
  /** Rows of every cell whose track assignment the edit touched, as committed. */
  affectedCells: z.array(CellRow),
  affectedTracks: z.array(u32),
})
export type GraphEditResult = z.infer<typeof GraphEditResult>

/** `/edits/undo|redo` answer with whichever domain the journal row carried. */
export const EditResult = z.union([MaskEditResult, GraphEditResult])
export type EditResult = z.infer<typeof EditResult>

export const LinkBody = z.object({ parentId: u32, childId: u32 })
export type LinkBody = z.infer<typeof LinkBody>

export const UnlinkBody = z.object({ cellId: u32 })
export type UnlinkBody = z.infer<typeof UnlinkBody>

export const Bbox = z.object({
  y0: z.number(),
  y1: z.number(),
  x0: z.number(),
  x1: z.number(),
})
export type Bbox = z.infer<typeof Bbox>

/** Raw pixel payloads are little-endian binary with metadata in headers, not JSON. */
export interface PlaneBuffer {
  shape: [height: number, width: number]
  channels: number
  dtype: Dtype
  level: number
  data: ArrayBuffer
}

export interface VolumeBuffer {
  shape: [z: number, y: number, x: number]
  dtype: Dtype
  level: number
  data: ArrayBuffer
}
