import type { Dims, PhysicalScale } from '@cellstudio/api-client'
import type { AxisScale, SliceOrientation } from '../state/nav'

/** Pixel coordinate, [z, y, x] — the order the database and the wire use. */
export type PixelZYX = readonly [z: number, y: number, x: number]
/** Render-space coordinate, [x, y, z] — the order deck.gl uses. */
export type WorldXYZ = readonly [x: number, y: number, z: number]

/**
 * px -> render-space conversion, normalized so one x pixel is one world unit: the XY
 * view keeps pixel coordinates while XZ/YZ/3D pick up the physical aspect. `axisScale`
 * (display-only) folds in here and nowhere else, so no reported coordinate ever sees it.
 */
export interface WorldTransform {
  /** World units per pixel, [x, y, z]. */
  unit: WorldXYZ
  /** True when voxel-size metadata was missing and the aspect fell back to isotropic. */
  isotropicFallback: boolean
  toWorld(px: PixelZYX): WorldXYZ
  fromWorld(w: WorldXYZ): PixelZYX
}

export function makeWorldTransform(
  scale: PhysicalScale | null,
  axisScale: AxisScale,
): WorldTransform {
  const isotropicFallback = scale === null || !(scale.x > 0 && scale.y > 0 && scale.z > 0)
  const physical = isotropicFallback ? { z: 1, y: 1, x: 1 } : (scale as PhysicalScale)
  const base = physical.x * axisScale.x
  const unit: WorldXYZ = [1, (physical.y * axisScale.y) / base, (physical.z * axisScale.z) / base]
  return {
    unit,
    isotropicFallback,
    toWorld: ([z, y, x]) => [x * unit[0], y * unit[1], z * unit[2]],
    fromWorld: ([x, y, z]) => [z / unit[2], y / unit[1], x / unit[0]],
  }
}

export const toWorld = (
  px: PixelZYX,
  scale: PhysicalScale | null,
  axisScale: AxisScale,
): WorldXYZ => makeWorldTransform(scale, axisScale).toWorld(px)

export const fromWorld = (
  w: WorldXYZ,
  scale: PhysicalScale | null,
  axisScale: AxisScale,
): PixelZYX => makeWorldTransform(scale, axisScale).fromWorld(w)

/** Which dataset axis each slice orientation puts on screen, and which one it steps along. */
export const sliceAxes = (
  o: SliceOrientation,
): { horizontal: 'x' | 'y'; vertical: 'y' | 'z'; normal: 'z' | 'y' | 'x' } =>
  o === 'xy'
    ? { horizontal: 'x', vertical: 'y', normal: 'z' }
    : o === 'xz'
      ? { horizontal: 'x', vertical: 'z', normal: 'y' }
      : { horizontal: 'y', vertical: 'z', normal: 'x' }

export interface Extent2D {
  /** World-space size of the plane. */
  width: number
  height: number
  /** Pixel size of the plane, for the image quad's texture dimensions. */
  pixelWidth: number
  pixelHeight: number
}

export function sliceExtent(o: SliceOrientation, dims: Dims, transform: WorldTransform): Extent2D {
  const axes = sliceAxes(o)
  const unitOf = { x: transform.unit[0], y: transform.unit[1], z: transform.unit[2] }
  return {
    width: dims[axes.horizontal] * unitOf[axes.horizontal],
    height: dims[axes.vertical] * unitOf[axes.vertical],
    pixelWidth: dims[axes.horizontal],
    pixelHeight: dims[axes.vertical],
  }
}

/** World-space box of the whole volume, [x, y, z]. */
/**
 * The 3D volume's frame. viv's `XR3DLayer` is written against viv's own loader, which
 * assembles the buffer with every z-plane's rows reversed, and its shader samples the
 * texture straight — so a volume packed that way draws voxel row 0 at `extent - 0`, not
 * at 0. Anything converting between voxels and 3D world space goes through this, or the
 * cursor and the voxels it writes end up mirrored about the volume's y centre.
 */
export function volumeFrame(transform: WorldTransform, dims: Dims): WorldTransform {
  const extentY = dims.y * transform.unit[1]
  return {
    ...transform,
    toWorld: (px) => {
      const [x, y, z] = transform.toWorld(px)
      return [x, extentY - y, z]
    },
    fromWorld: ([x, y, z]) => transform.fromWorld([x, extentY - y, z]),
  }
}

export const volumeExtent = (dims: Dims, transform: WorldTransform): WorldXYZ => [
  dims.x * transform.unit[0],
  dims.y * transform.unit[1],
  dims.z * transform.unit[2],
]

export interface Viewport2D {
  width: number
  height: number
}

export interface Fit {
  /** deck.gl OrthographicView zoom: world units scale by 2**zoom. */
  zoom: number
  target: [number, number]
}

const MIN_EXTENT = 1e-6

/**
 * Fit-to-viewport for a slice quad. A thin-Z plane (3 px tall) fits by zooming in
 * hard rather than degenerating, so the guard is on zero extent, not on aspect.
 */
export function fitSlice(extent: Extent2D, viewport: Viewport2D): Fit {
  const width = Math.max(extent.width, MIN_EXTENT)
  const height = Math.max(extent.height, MIN_EXTENT)
  const vw = Math.max(viewport.width, 1)
  const vh = Math.max(viewport.height, 1)
  return {
    zoom: Math.log2(Math.min(vw / width, vh / height)),
    target: [width / 2, height / 2],
  }
}

/**
 * Screen-space point on a slice quad back to dataset pixels. Display scaling divides out
 * here, which is why the cursor readout and hit-tests never see it.
 */
export function pixelFromSliceWorld(
  o: SliceOrientation,
  index: number,
  world: readonly [number, number],
  transform: WorldTransform,
): PixelZYX {
  const axes = sliceAxes(o)
  const unitOf = { x: transform.unit[0], y: transform.unit[1], z: transform.unit[2] }
  const h = world[0] / unitOf[axes.horizontal]
  const v = world[1] / unitOf[axes.vertical]
  if (o === 'xy') return [index, v, h]
  if (o === 'xz') return [v, index, h]
  return [v, h, index]
}

/** The inverse: dataset pixels to the slice quad's two on-screen axes. */
export function sliceWorldFromPixel(
  o: SliceOrientation,
  px: PixelZYX,
  transform: WorldTransform,
): [number, number] {
  const axes = sliceAxes(o)
  const unitOf = { x: transform.unit[0], y: transform.unit[1], z: transform.unit[2] }
  const value = { z: px[0], y: px[1], x: px[2] }
  return [
    value[axes.horizontal] * unitOf[axes.horizontal],
    value[axes.vertical] * unitOf[axes.vertical],
  ]
}

/**
 * Orbit fit. Matches viv's `getDefaultInitialViewState`: zoom from the XY extent with a
 * back-off, orbit target at the volume centre.
 */
export function fitVolume(
  extent: WorldXYZ,
  viewport: Viewport2D,
  zoomBackOff = 0.5,
): Fit & { target3d: WorldXYZ } {
  const width = Math.max(extent[0], MIN_EXTENT)
  const height = Math.max(extent[1], MIN_EXTENT)
  const vw = Math.max(viewport.width, 1)
  const vh = Math.max(viewport.height, 1)
  return {
    zoom: Math.log2(Math.min(vw / width, vh / height)) - zoomBackOff,
    target: [extent[0] / 2, extent[1] / 2],
    target3d: [extent[0] / 2, extent[1] / 2, extent[2] / 2],
  }
}
