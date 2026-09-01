import { COORDINATE_SYSTEM, CompositeLayer, type Layer } from '@deck.gl/core'
import { PathLayer, ScatterplotLayer } from '@deck.gl/layers'
import type { CellRow } from '@cellstudio/api-client'
import type { SliceOrientation } from '../state/nav'
import { sliceAxes, type WorldTransform } from '../data/world'
import { labelColor } from './labelPalette'

export type Rgb = [number, number, number]

/**
 * Trail color = the label palette entry for the track id (`trackPaletteIndex` keying), so
 * a mask remapped to track ids and its trail agree by construction. */
export function trackColor(trackId: number): Rgb {
  return labelColor(trackId)
}

export const SELECTED_COLOR: Rgb = [255, 255, 255]

/** Trail opacity decay: linear from `max` at the current frame to `min` at the window start. */
export interface TrackFade {
  on: boolean
  max: number
  min: number
}

export const DEFAULT_TRACK_FADE: TrackFade = { on: true, max: 1, min: 0.15 }

/**
 * The selected lineage as the scenes receive it. the highlight set comes from
 * `cells`, and division edges — links whose endpoints carry different track ids, which
 * `buildTracks` can never invent from ids alone — come from the explicit `links`.
 * `graphVersion` is the version the tree was fetched under; a stale overlay is dropped.
 */
export interface LineageOverlay {
  graphVersion: number
  focusCellId: number
  cells: readonly CellRow[]
  links: readonly { parent: number; child: number }[]
}

/** Exact `max` at f = 1 and `min` at f = 0, immune to float drift in the lerp. */
export const trailAlpha = (segT: number, t: number, trail: number, fade: TrackFade): number => {
  if (!fade.on || trail <= 0) return fade.max
  const f = (segT - (t - trail)) / trail
  return f >= 1 ? fade.max : f <= 0 ? fade.min : fade.min + (fade.max - fade.min) * f
}

/** Cells within the backward window [t − trail, t]; nothing from future frames. */
export const inTrailWindow = (cells: readonly CellRow[], t: number, trail: number): CellRow[] =>
  cells.filter((c) => c.t >= t - trail && c.t <= t)

/**
 * Slab filter for a slice view: |centroid along the view normal − slice index| ≤ radius.
 * Centroids are pixel coordinates, so display scaling never changes what is in the slab.
 */
export function inSlab(
  cells: readonly CellRow[],
  orientation: SliceOrientation,
  index: number,
  radius: number,
): CellRow[] {
  const axis = sliceAxes(orientation).normal
  const at = axis === 'z' ? 0 : axis === 'y' ? 1 : 2
  return cells.filter(
    (c) => c.centroid !== null && Math.abs((c.centroid[at] as number) - index) <= radius,
  )
}

/** A track's extent over the loaded rows, and the track its head cell descends from. */
export interface TrackSpan {
  first: number
  last: number
  parent: number | null
}

const trackOf = (c: CellRow): number => c.trackId ?? c.id

/** Every loaded row counts, future frames included: a gap at `t` must not end a track. */
export function trackSpans(cells: readonly CellRow[]): Map<number, TrackSpan> {
  const byId = new Map<number, CellRow>()
  for (const c of cells) byId.set(c.id, c)
  const spans = new Map<number, TrackSpan>()
  for (const c of cells) {
    const id = trackOf(c)
    const span = spans.get(id)
    if (span) {
      if (c.t < span.first) span.first = c.t
      if (c.t > span.last) span.last = c.t
    } else {
      spans.set(id, { first: c.t, last: c.t, parent: null })
    }
    const parent = c.parentId === null ? undefined : byId.get(c.parentId)
    if (parent && trackOf(parent) !== id) (spans.get(id) as TrackSpan).parent = trackOf(parent)
  }
  return spans
}

/**
 * Tracks whose trails draw at frame `t`: those alive at `t` (first ≤ t ≤ last over the
 * loaded rows) plus every ancestor track, since a daughter's history runs on through the
 * frames of the track it divided from. A track that ended before `t` and left no living
 * descendant draws nothing.
 */
export function shownTracks(spans: ReadonlyMap<number, TrackSpan>, t: number): Set<number> {
  const shown = new Set<number>()
  for (const [id, span] of spans) {
    if (span.first > t || span.last < t) continue
    let cursor: number | null = id
    // the `has` guard also stops a degenerate cycle from looping forever
    while (cursor !== null && !shown.has(cursor)) {
      shown.add(cursor)
      cursor = spans.get(cursor)?.parent ?? null
    }
  }
  return shown
}

export interface TrackPoint {
  cellId: number
  trackId: number
  t: number
  position: number[]
  color: Rgb
  current: boolean
  selected: boolean
}

export interface TrackSegment {
  trackId: number
  fromCellId: number
  toCellId: number
  path: number[][]
  color: Rgb
  alpha: number
  selected: boolean
}

export interface BuildTracksArgs {
  cells: readonly CellRow[]
  t: number
  trail: number
  transform: WorldTransform
  /** Omit for 3D: positions come out as [x, y, z] instead of the slice's two axes. */
  orientation?: SliceOrientation
  index?: number
  slab?: number
  /** Cell ids of the selected lineage, highlighted distinctly. */
  lineage?: ReadonlySet<number>
  fade?: TrackFade
}

export interface BuiltTracks {
  points: TrackPoint[]
  segments: TrackSegment[]
}

const project = (
  position: readonly [number, number, number],
  orientation?: SliceOrientation,
): number[] => {
  if (!orientation) return [...position]
  const axes = sliceAxes(orientation)
  const pick = { x: position[0], y: position[1], z: position[2] }
  return [pick[axes.horizontal], pick[axes.vertical]]
}

/**
 * Trails and centroids for the backward time window [t − trail, t], for the tracks
 * `shownTracks` admits: one segment per consecutive pair so trails can decay with age,
 * a segment from a parent's last cell into each daughter's first so a daughter's trail
 * continues into its parent track, plus emphasized centroids at the current frame. A
 * segment's time is its newer endpoint, so the segment ending at the current frame
 * renders at exactly `fade.max` and the one ending at t − trail at exactly `fade.min`.
 */
export function buildTracks(args: BuildTracksArgs): BuiltTracks {
  const { cells, t, trail, transform, orientation, index, slab, lineage } = args
  const fade = args.fade ?? DEFAULT_TRACK_FADE
  const shown = shownTracks(trackSpans(cells), t)
  const windowed = inTrailWindow(cells, t, trail).filter((c) => shown.has(trackOf(c)))
  const visible =
    orientation && index !== undefined
      ? inSlab(windowed, orientation, index, slab ?? trail)
      : windowed

  const points: TrackPoint[] = []
  for (const cell of visible) {
    if (cell.centroid === null) continue
    const trackId = trackOf(cell)
    points.push({
      cellId: cell.id,
      trackId,
      t: cell.t,
      position: project(transform.toWorld(cell.centroid), orientation),
      color: lineage?.has(cell.id) ? SELECTED_COLOR : trackColor(trackId),
      current: cell.t === t,
      selected: lineage?.has(cell.id) ?? false,
    })
  }

  // Segments group all past cells so the pair whose newer endpoint sits exactly at
  // t − trail still renders (its older endpoint lies just outside the window).
  const byId = new Map<number, CellRow>()
  const byTrack = new Map<number, CellRow[]>()
  for (const cell of cells) {
    if (cell.t > t || cell.centroid === null) continue
    byId.set(cell.id, cell)
    const trackId = trackOf(cell)
    if (!shown.has(trackId)) continue
    const list = byTrack.get(trackId)
    if (list) list.push(cell)
    else byTrack.set(trackId, [cell])
  }

  const segments: TrackSegment[] = []
  const segment = (from: CellRow, to: CellRow): void => {
    if (to.t < t - trail) return
    const inSlabEither =
      !orientation ||
      index === undefined ||
      inSlab([from, to], orientation, index, slab ?? trail).length > 0
    if (!inSlabEither) return
    const selected = (lineage?.has(from.id) || lineage?.has(to.id)) ?? false
    segments.push({
      trackId: trackOf(to),
      fromCellId: from.id,
      toCellId: to.id,
      path: [
        project(transform.toWorld(from.centroid as [number, number, number]), orientation),
        project(transform.toWorld(to.centroid as [number, number, number]), orientation),
      ],
      color: selected ? SELECTED_COLOR : trackColor(trackOf(to)),
      alpha: trailAlpha(to.t, t, trail, fade),
      selected,
    })
  }
  for (const list of byTrack.values()) {
    list.sort((a, b) => a.t - b.t)
    for (let i = 1; i < list.length; i += 1) segment(list[i - 1] as CellRow, list[i] as CellRow)
    // the division edge: this track's head back to the parent cell it descends from
    const head = list[0] as CellRow
    const parent = head.parentId === null ? undefined : byId.get(head.parentId)
    if (parent && trackOf(parent) !== trackOf(head)) segment(parent, head)
  }
  return { points, segments }
}

export interface BuildLineageEdgesArgs {
  lineage: LineageOverlay | null | undefined
  t: number
  trail: number
  transform: WorldTransform
  orientation?: SliceOrientation
  index?: number
  slab?: number
  fade?: TrackFade
  /** Tracks `buildTracks` draws; an edge into a track outside it is dropped. */
  shown?: ReadonlySet<number>
}

/**
 * Division edges of the selected lineage: segments for the overlay's cross-track links,
 * under the same backward window (segment time = the child endpoint) and slab filter as
 * the trails. Same-track links are already drawn by `buildTracks`.
 */
export function buildLineageEdges(args: BuildLineageEdgesArgs): TrackSegment[] {
  const { lineage, t, trail, transform, orientation, index, slab, shown } = args
  if (!lineage) return []
  const fade = args.fade ?? DEFAULT_TRACK_FADE
  const byId = new Map(lineage.cells.map((c) => [c.id, c]))
  const segments: TrackSegment[] = []
  for (const link of lineage.links) {
    const from = byId.get(link.parent)
    const to = byId.get(link.child)
    if (!from || !to || from.centroid === null || to.centroid === null) continue
    if (trackOf(from) === trackOf(to)) continue
    if (shown && !shown.has(trackOf(to))) continue
    if (to.t > t || to.t < t - trail) continue
    const inSlabEither =
      !orientation ||
      index === undefined ||
      inSlab([from, to], orientation, index, slab ?? trail).length > 0
    if (!inSlabEither) continue
    segments.push({
      trackId: trackOf(to),
      fromCellId: from.id,
      toCellId: to.id,
      path: [
        project(transform.toWorld(from.centroid), orientation),
        project(transform.toWorld(to.centroid), orientation),
      ],
      color: SELECTED_COLOR,
      alpha: trailAlpha(to.t, t, trail, fade),
      selected: true,
    })
  }
  return segments
}

export interface TrackLayerProps {
  cells: readonly CellRow[]
  t: number
  trail: number
  transform: WorldTransform
  orientation: SliceOrientation
  index: number
  /** Slab half-thickness in pixels along the view normal. */
  slab: number
  /** The selected trail edge, drawn white and thick. */
  selectedLink?: { parent: number; child: number } | null
  /** Centroid radius in image pixels; scales with the image, not the screen. */
  dotSize?: number
  lineage?: ReadonlySet<number>
  lineageOverlay?: LineageOverlay
  trackOpacity?: number
  fade?: TrackFade
}

/**
 * Adds the overlay's division edges that `buildTracks` did not already draw from the
 * rows — an edge whose parent cell lies outside the loaded window — under its gate.
 */
export function withLineageEdges(
  built: BuiltTracks,
  cells: readonly CellRow[],
  args: Omit<BuildLineageEdgesArgs, 'shown'>,
): BuiltTracks {
  if (!args.lineage) return built
  const shown = shownTracks(trackSpans(cells), args.t)
  const drawn = new Set(built.segments.map((s) => `${s.fromCellId}>${s.toCellId}`))
  const edges = buildLineageEdges({ ...args, shown }).filter(
    (e) => !drawn.has(`${e.fromCellId}>${e.toCellId}`),
  )
  return edges.length > 0 ? { ...built, segments: built.segments.concat(edges) } : built
}

const SEGMENT_WIDTH = 1.6
/** Centroid radius in image pixels; the current frame's dot is this much larger. */
export const DEFAULT_DOT_SIZE = 3
const CURRENT_DOT_FACTOR = 1.5

/** Shared sublayer construction: fading trails under emphasized centroids. */
abstract class TrackOverlayLayer<
  PropsT extends {
    trackOpacity?: number
    dotSize?: number
    parameters?: Record<string, unknown>
    selectedLink?: { parent: number; child: number } | null
  },
> extends CompositeLayer<PropsT> {
  /** `dot` is the radius in world units — image pixels scaled by the view's unit. */
  protected trackSublayers(built: BuiltTracks, dot: number): Layer[] {
    const opacity = this.props.trackOpacity ?? 1
    const edge = this.props.selectedLink
    const isEdge = (s: TrackSegment) =>
      edge !== undefined &&
      edge !== null &&
      s.fromCellId === edge.parent &&
      s.toCellId === edge.child
    // the 3D scene passes depthTest: false, or the volume's depth buffer hides every
    // centroid and trail inside the box (the same reason the brush orb does)
    const parameters = this.props.parameters
    return [
      new PathLayer(
        this.getSubLayerProps({
          id: 'trails',
          data: built.segments,
          coordinateSystem: COORDINATE_SYSTEM.CARTESIAN,
          ...(parameters ? { parameters } : {}),
          widthUnits: 'pixels',
          // 2 px minimum so a thin trail is still clickable
          widthMinPixels: 2,
          // a click on a trail selects that one edge, so Unlink can cut it
          pickable: true,
          getPath: (s: TrackSegment) => s.path,
          getColor: (s: TrackSegment) =>
            isEdge(s)
              ? [...SELECTED_COLOR, 255]
              : [...s.color, Math.round(255 * opacity * s.alpha)],
          getWidth: (s: TrackSegment) =>
            isEdge(s) ? SEGMENT_WIDTH * 3 : s.selected ? SEGMENT_WIDTH * 2 : SEGMENT_WIDTH,
          updateTriggers: {
            getColor: [opacity, edge],
            getWidth: [edge],
            getPath: [built.segments],
          },
        }),
      ),
      new ScatterplotLayer(
        this.getSubLayerProps({
          id: 'centroids',
          data: built.points,
          coordinateSystem: COORDINATE_SYSTEM.CARTESIAN,
          ...(parameters ? { parameters } : {}),
          // world-unit radii: a dot keeps its size relative to the image at every zoom,
          // with a 1 px floor so it never vanishes zoomed out. Billboarded so the 3D
          // camera sees a circle rather than a disc lying in the world XY plane.
          radiusUnits: 'common',
          radiusMinPixels: 1,
          radiusScale: 1,
          billboard: true,
          pickable: true,
          stroked: true,
          lineWidthUnits: 'pixels',
          getPosition: (p: TrackPoint) => p.position,
          getRadius: (p: TrackPoint) => (p.current ? dot * CURRENT_DOT_FACTOR : dot),
          getFillColor: (p: TrackPoint) => [
            ...p.color,
            Math.round(255 * opacity * (p.current ? 1 : 0.55)),
          ],
          getLineColor: (p: TrackPoint) => (p.selected ? SELECTED_COLOR : p.color),
          getLineWidth: (p: TrackPoint) => (p.selected ? 2 : 0),
          updateTriggers: { getFillColor: [opacity], getRadius: [built.points, dot] },
        }),
      ),
    ]
  }
}

/** Trails and centroids in a slice view, slab-filtered around the current plane. */
export class TrackLayer extends TrackOverlayLayer<TrackLayerProps> {
  static override layerName = 'TrackLayer'

  override renderLayers(): Layer[] {
    const { cells, t, trail, transform, orientation, index, slab, lineage, lineageOverlay, fade } =
      this.props
    const built = withLineageEdges(
      buildTracks({ cells, t, trail, transform, orientation, index, slab, lineage, fade }),
      cells,
      { lineage: lineageOverlay, t, trail, transform, orientation, index, slab, fade },
    )
    // image pixels → this view's world units, so the size the user set means the same
    // thing whatever the display scale is (`unit` is [x, y, z], x normalized to 1)
    const unitOf = { x: transform.unit[0], y: transform.unit[1], z: transform.unit[2] }
    const unit = unitOf[sliceAxes(orientation).horizontal]
    return this.trackSublayers(built, (this.props.dotSize ?? DEFAULT_DOT_SIZE) * unit)
  }
}

export interface TrackLayer3DProps {
  cells: readonly CellRow[]
  t: number
  trail: number
  /** Centroid radius in image pixels; scales with the image, not the screen. */
  dotSize?: number
  /** The selected trail edge, drawn white and thick. */
  selectedLink?: { parent: number; child: number } | null
  transform: WorldTransform
  lineage?: ReadonlySet<number>
  lineageOverlay?: LineageOverlay
  trackOpacity?: number
  fade?: TrackFade
  /** Render parameters for the sublayers; the 3D scene disables the depth test. */
  parameters?: Record<string, unknown>
}

/** The same overlay in the 3D scene, positioned in physical space via `toWorld`. */
export class TrackLayer3D extends TrackOverlayLayer<TrackLayer3DProps> {
  static override layerName = 'TrackLayer3D'

  override renderLayers(): Layer[] {
    const { cells, t, trail, transform, lineage, lineageOverlay, fade } = this.props
    const built = withLineageEdges(
      buildTracks({ cells, t, trail, transform, lineage, fade }),
      cells,
      { lineage: lineageOverlay, t, trail, transform, fade },
    )
    return this.trackSublayers(built, (this.props.dotSize ?? DEFAULT_DOT_SIZE) * transform.unit[0])
  }
}
