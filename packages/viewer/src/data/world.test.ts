import { describe, expect, it } from 'vitest'
import {
  fitSlice,
  fitVolume,
  makeWorldTransform,
  pixelFromSliceWorld,
  sliceExtent,
  sliceWorldFromPixel,
  volumeExtent,
  volumeFrame,
} from './world'
import { devProject } from '../test/data'

const scale = { z: 2.0, y: 0.603, x: 0.603 }
const stretchZ = { z: 8, y: 1, x: 1 }

describe('toWorld and its inverse', () => {
  it('round-trips pixel coordinates under non-unit display scaling', () => {
    const transform = makeWorldTransform(scale, { z: 8, y: 1.7, x: 0.4 })
    for (const px of [
      [0, 0, 0],
      [1, 511, 1023],
      [2.5, 100.25, 3.75],
    ] as const) {
      const back = transform.fromWorld(transform.toWorld(px))
      expect(back[0]).toBeCloseTo(px[0], 10)
      expect(back[1]).toBeCloseTo(px[1], 10)
      expect(back[2]).toBeCloseTo(px[2], 10)
    }
  })

  it('normalizes to x so anisotropy shows up on z, not on xy', () => {
    const transform = makeWorldTransform(scale, { z: 1, y: 1, x: 1 })
    expect(transform.unit[0]).toBe(1)
    expect(transform.unit[1]).toBeCloseTo(1, 12)
    expect(transform.unit[2]).toBeCloseTo(2.0 / 0.603, 12)
  })

  it('multiplies the z display scale into the z unit only', () => {
    const plain = makeWorldTransform(scale, { z: 1, y: 1, x: 1 })
    const stretched = makeWorldTransform(scale, stretchZ)
    expect(stretched.unit[2]).toBeCloseTo(plain.unit[2] * 8, 12)
    expect(stretched.unit[0]).toBe(plain.unit[0])
    expect(stretched.unit[1]).toBeCloseTo(plain.unit[1], 12)
  })

  it('falls back to isotropic when voxel size is missing', () => {
    const transform = makeWorldTransform(null, { z: 1, y: 1, x: 1 })
    expect(transform.isotropicFallback).toBe(true)
    expect(transform.unit).toEqual([1, 1, 1])
    expect(makeWorldTransform({ z: 0, y: 1, x: 1 }, { z: 1, y: 1, x: 1 }).isotropicFallback).toBe(
      true,
    )
  })

  it('round-trips a point on a slice quad back to dataset pixels', () => {
    const transform = makeWorldTransform(scale, { z: 4, y: 1, x: 1 })
    for (const o of ['xy', 'xz', 'yz'] as const) {
      const px = [2, 300, 700] as const
      const world = sliceWorldFromPixel(o, px, transform)
      const index = o === 'xy' ? px[0] : o === 'xz' ? px[1] : px[2]
      const back = pixelFromSliceWorld(o, index, world, transform)
      expect(back[0]).toBeCloseTo(px[0], 10)
      expect(back[1]).toBeCloseTo(px[1], 10)
      expect(back[2]).toBeCloseTo(px[2], 10)
    }
  })
})

describe('thin-Z geometry', () => {
  const dims = devProject().dims // z = 3

  it('gives XZ a finite non-zero extent at the physical aspect', () => {
    const transform = makeWorldTransform(scale, { z: 1, y: 1, x: 1 })
    const extent = sliceExtent('xz', dims, transform)
    expect(extent.pixelHeight).toBe(3)
    expect(extent.width).toBe(1024)
    expect(extent.height).toBeCloseTo(3 * (2.0 / 0.603), 10)
    expect(extent.height).toBeGreaterThan(0)
    expect(Number.isFinite(extent.height)).toBe(true)
  })

  it('fits a 3-plane XZ view into the viewport with finite zoom', () => {
    const transform = makeWorldTransform(scale, { z: 1, y: 1, x: 1 })
    const fit = fitSlice(sliceExtent('xz', dims, transform), { width: 1200, height: 800 })
    expect(Number.isFinite(fit.zoom)).toBe(true)
    expect(fit.zoom).toBeCloseTo(Math.log2(1200 / 1024), 10)
    expect(fit.target[1]).toBeGreaterThan(0)
  })

  it('never divides by a zero extent', () => {
    const flat = { t: 1, c: 1, z: 0, y: 0, x: 0 }
    const fit = fitSlice(sliceExtent('xz', flat, makeWorldTransform(scale, { z: 1, y: 1, x: 1 })), {
      width: 800,
      height: 600,
    })
    expect(Number.isFinite(fit.zoom)).toBe(true)
  })

  it('keeps the thin-Z volume box non-degenerate and stretchable', () => {
    const plain = volumeExtent(dims, makeWorldTransform(scale, { z: 1, y: 1, x: 1 }))
    const stretched = volumeExtent(dims, makeWorldTransform(scale, stretchZ))
    expect(plain[2]).toBeCloseTo(3 * (2.0 / 0.603), 10)
    expect(stretched[2]).toBeCloseTo(plain[2] * 8, 10)
    expect(stretched[0]).toBe(plain[0])
    const fit = fitVolume(stretched, { width: 900, height: 900 })
    expect(Number.isFinite(fit.zoom)).toBe(true)
    expect(fit.target3d[2]).toBeCloseTo(stretched[2] / 2, 10)
  })
})

describe('volumeFrame', () => {
  // 3 z-planes, 1024 square, the development dataset's anisotropy
  const dims = { t: 277, c: 3, z: 3, y: 1024, x: 1024 }
  const transform = makeWorldTransform(
    { z: 2.0, y: 0.60296875, x: 0.6029296875 },
    { z: 1, y: 1, x: 1 },
  )
  const frame = volumeFrame(transform, dims)

  it('mirrors y, because viv draws voxel row 0 at the far end of the volume', () => {
    const extentY = dims.y * transform.unit[1]
    const [, worldY] = frame.toWorld([1, 0, 512])
    expect(worldY).toBeCloseTo(extentY, 6)
    expect(frame.toWorld([1, dims.y, 512])[1]).toBeCloseTo(0, 6)
  })

  it('round-trips, so the orb writes the voxel the cursor is over', () => {
    const voxel: [number, number, number] = [1, 492, 409]
    const back = frame.fromWorld(frame.toWorld(voxel))
    expect(back[0]).toBeCloseTo(voxel[0], 6)
    expect(back[1]).toBeCloseTo(voxel[1], 6)
    expect(back[2]).toBeCloseTo(voxel[2], 6)
  })

  it('leaves x and z alone', () => {
    const [x, , z] = frame.toWorld([1, 492, 409])
    const [px, , pz] = transform.toWorld([1, 492, 409])
    expect(x).toBe(px)
    expect(z).toBe(pz)
  })
})
