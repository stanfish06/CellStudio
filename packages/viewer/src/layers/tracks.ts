import { COORDINATE_SYSTEM, CompositeLayer, type Layer } from '@deck.gl/core'
import { PathLayer, ScatterplotLayer } from '@deck.gl/layers'
import type { CellRow } from '@cellstudio/api-client'
import type { SliceOrientation } from '../state/nav'
import { sliceAxes, type WorldTransform } from '../data/world'

export type Rgb = [number, number, number]

/** Deterministic hue per track id, so colors are identical across sessions. */
export function trackColor(trackId: number): Rgb {
  const hue = (trackId * 137.508) % 360
  const c = 0.62
  const x = c * (1 - Math.abs(((hue / 60) % 2) - 1))
  const m = 0.32
  const [r, g, b] =
    hue < 60
      ? [c, x, 0]
      : hue < 120
        ? [x, c, 0]
        : hue < 180
          ? [0, c, x]
          : hue < 240
            ? [0, x, c]
            : hue < 300
              ? [x, 0, c]
              : [c, 0, x]
  return [
    Math.round(((r as number) + m) * 255),
    Math.round(((g as number) + m) * 255),
    Math.round(((b as number) + m) * 255),
  ]
}

export const SELECTED_COLOR: Rgb = [255, 255, 255]

/** Cells within ±`trail` frames of the current one. */
export const inTrailWindow = (cells: readonly CellRow[], t: number, trail: number): CellRow[] =>
  cells.filter((c) => Math.abs(c.t - t) <= trail)

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
 * Trails and centroids for the current time window: one segment per consecutive pair so
 * trails can fade with distance in t, plus emphasized centroids at the current frame.
 */
export function buildTracks(args: BuildTracksArgs): BuiltTracks {
  const { cells, t, trail, transform, orientation, index, slab, lineage } = args
  const windowed = inTrailWindow(cells, t, trail)
  const visible =
    orientation && index !== undefined
      ? inSlab(windowed, orientation, index, slab ?? trail)
      : windowed

  const points: TrackPoint[] = []
  for (const cell of visible) {
    if (cell.centroid === null) continue
    const trackId = cell.trackId ?? cell.id
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

  const byTrack = new Map<number, CellRow[]>()
  for (const cell of windowed) {
    if (cell.centroid === null) continue
    const trackId = cell.trackId ?? cell.id
    const list = byTrack.get(trackId)
    if (list) list.push(cell)
    else byTrack.set(trackId, [cell])
  }

  const segments: TrackSegment[] = []
  const span = Math.max(1, trail + 1)
  for (const [trackId, list] of byTrack) {
    list.sort((a, b) => a.t - b.t)
    for (let i = 1; i < list.length; i += 1) {
      const from = list[i - 1] as CellRow
      const to = list[i] as CellRow
      const inSlabBoth =
        !orientation ||
        index === undefined ||
        inSlab([from, to], orientation, index, slab ?? trail).length > 0
      if (!inSlabBoth) continue
      const mid = (from.t + to.t) / 2
      const selected = (lineage?.has(from.id) || lineage?.has(to.id)) ?? false
      segments.push({
        trackId,
        fromCellId: from.id,
        toCellId: to.id,
        path: [
          project(transform.toWorld(from.centroid as [number, number, number]), orientation),
          project(transform.toWorld(to.centroid as [number, number, number]), orientation),
        ],
        color: selected ? SELECTED_COLOR : trackColor(trackId),
        alpha: Math.max(0.15, 1 - Math.abs(mid - t) / span),
        selected,
      })
    }
  }
  return { points, segments }
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
  lineage?: ReadonlySet<number>
  trackOpacity?: number
}

const SEGMENT_WIDTH = 1.6
const POINT_RADIUS = 4
const CURRENT_POINT_RADIUS = 6

/** Shared sublayer construction: fading trails under emphasized centroids. */
abstract class TrackOverlayLayer<
  PropsT extends { trackOpacity?: number },
> extends CompositeLayer<PropsT> {
  protected trackSublayers(built: BuiltTracks): Layer[] {
    const opacity = this.props.trackOpacity ?? 1
    return [
      new PathLayer(
        this.getSubLayerProps({
          id: 'trails',
          data: built.segments,
          coordinateSystem: COORDINATE_SYSTEM.CARTESIAN,
          widthUnits: 'pixels',
          widthMinPixels: 1,
          getPath: (s: TrackSegment) => s.path,
          getColor: (s: TrackSegment) => [...s.color, Math.round(255 * opacity * s.alpha)],
          getWidth: (s: TrackSegment) => (s.selected ? SEGMENT_WIDTH * 2 : SEGMENT_WIDTH),
          updateTriggers: { getColor: [opacity], getPath: [built.segments] },
        }),
      ),
      new ScatterplotLayer(
        this.getSubLayerProps({
          id: 'centroids',
          data: built.points,
          coordinateSystem: COORDINATE_SYSTEM.CARTESIAN,
          radiusUnits: 'pixels',
          pickable: true,
          stroked: true,
          lineWidthUnits: 'pixels',
          getPosition: (p: TrackPoint) => p.position,
          getRadius: (p: TrackPoint) => (p.current ? CURRENT_POINT_RADIUS : POINT_RADIUS),
          getFillColor: (p: TrackPoint) => [
            ...p.color,
            Math.round(255 * opacity * (p.current ? 1 : 0.55)),
          ],
          getLineColor: (p: TrackPoint) => (p.selected ? SELECTED_COLOR : p.color),
          getLineWidth: (p: TrackPoint) => (p.selected ? 2 : 0),
          updateTriggers: { getFillColor: [opacity], getRadius: [built.points] },
        }),
      ),
    ]
  }
}

/** Trails and centroids in a slice view, slab-filtered around the current plane. */
export class TrackLayer extends TrackOverlayLayer<TrackLayerProps> {
  static override layerName = 'TrackLayer'

  override renderLayers(): Layer[] {
    const { cells, t, trail, transform, orientation, index, slab, lineage } = this.props
    const built = buildTracks({ cells, t, trail, transform, orientation, index, slab, lineage })
    return this.trackSublayers(built)
  }
}

export interface TrackLayer3DProps {
  cells: readonly CellRow[]
  t: number
  trail: number
  transform: WorldTransform
  lineage?: ReadonlySet<number>
  trackOpacity?: number
}

/** The same overlay in the 3D scene, positioned in physical space via `toWorld`. */
export class TrackLayer3D extends TrackOverlayLayer<TrackLayer3DProps> {
  static override layerName = 'TrackLayer3D'

  override renderLayers(): Layer[] {
    const { cells, t, trail, transform, lineage } = this.props
    const built = buildTracks({ cells, t, trail, transform, lineage })
    return this.trackSublayers(built)
  }
}
