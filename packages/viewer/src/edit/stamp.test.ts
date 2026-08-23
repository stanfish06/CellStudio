import { describe, expect, it } from 'vitest'
import {
  downsample,
  stampHash,
  stampVoxels,
  voxelBounds,
  voxelCount,
  type StampAxis,
  type VoxelSet,
} from './stamp'

/**
 * The stamp contract. The same cases and expectations as
 * `crates/cellstudio-core/tests/labels.rs`, which is the point: the echo the renderer draws
 * and the voxels the server writes have to be the same set. The hash is over the sorted
 * coordinates, so it catches an interior disagreement that a matching count and bounding box
 * would hide. Changing the stamp formula means updating both sides together (design M5).
 */
interface Case {
  name: string
  dims: [number, number, number]
  centre: [number, number, number]
  radius: number
  scale: { z: number; y: number; x: number } | null
  plane: { axis: StampAxis; index: number } | null
  count: number
  bounds: number[] | null
  hash: number
  /** Point-sampled to a coarser level: factor, then the same three expectations. */
  coarse: [[number, number, number], number, number[] | null, number][]
}

const CASES: Case[] = [
  {
    name: 'fractional-centre',
    dims: [8, 32, 32],
    centre: [3.5, 12.25, 15.75],
    radius: 3.0,
    scale: null,
    plane: null,
    count: 106,
    bounds: [1, 5, 9, 14, 13, 18],
    hash: 737211305,
    coarse: [[[1, 2, 2], 29, [1, 5, 5, 7, 7, 9], 3362287568]],
  },
  {
    name: 'radius-one',
    dims: [8, 32, 32],
    centre: [4.5, 16.5, 16.5],
    radius: 1.0,
    scale: null,
    plane: null,
    count: 7,
    bounds: [3, 5, 15, 17, 15, 17],
    hash: 2679594327,
    coarse: [[[1, 2, 2], 3, [3, 5, 8, 8, 8, 8], 113349655]],
  },
  {
    name: 'centre-on-voxel-boundary',
    dims: [8, 32, 32],
    centre: [4.0, 16.0, 16.0],
    radius: 2.5,
    scale: null,
    plane: null,
    count: 56,
    bounds: [2, 5, 14, 17, 14, 17],
    hash: 226855493,
    coarse: [[[2, 2, 2], 7, [1, 2, 7, 8, 7, 8], 2617904996]],
  },
  {
    name: 'anisotropic',
    dims: [8, 32, 32],
    centre: [4.5, 16.5, 16.5],
    radius: 6.0,
    scale: { z: 2.0, y: 0.6, x: 0.6 },
    plane: null,
    count: 251,
    bounds: [3, 5, 10, 22, 10, 22],
    hash: 2381416567,
    coarse: [
      [[1, 3, 3], 29, [3, 5, 4, 7, 4, 7], 3448608705],
      [[1, 2, 2], 71, [3, 5, 5, 11, 5, 11], 1727368983],
    ],
  },
  {
    name: 'plane-z',
    dims: [8, 32, 32],
    centre: [4.5, 16.5, 16.5],
    radius: 5.0,
    scale: { z: 2.0, y: 0.6, x: 0.6 },
    plane: { axis: 'z', index: 4 },
    count: 81,
    bounds: [4, 4, 11, 21, 11, 21],
    hash: 2295155377,
    coarse: [[[1, 2, 2], 21, [4, 4, 6, 10, 6, 10], 3553274961]],
  },
  {
    name: 'plane-y',
    dims: [8, 32, 32],
    centre: [4.5, 16.5, 16.5],
    radius: 5.0,
    scale: { z: 2.0, y: 0.6, x: 0.6 },
    plane: { axis: 'y', index: 16 },
    count: 25,
    bounds: [3, 5, 16, 16, 11, 21],
    hash: 3253904173,
    coarse: [[[1, 2, 2], 11, [3, 5, 8, 8, 6, 10], 69718805]],
  },
  {
    name: 'plane-x',
    dims: [8, 32, 32],
    centre: [4.5, 16.5, 16.5],
    radius: 5.0,
    scale: { z: 2.0, y: 0.6, x: 0.6 },
    plane: { axis: 'x', index: 16 },
    count: 25,
    bounds: [3, 5, 11, 21, 16, 16],
    hash: 1557389101,
    coarse: [[[1, 2, 2], 11, [3, 5, 6, 10, 8, 8], 1545221237]],
  },
  {
    name: 'plane-z-off-slice',
    dims: [8, 32, 32],
    centre: [1.5, 16.5, 16.5],
    radius: 5.0,
    scale: { z: 2.0, y: 0.6, x: 0.6 },
    plane: { axis: 'z', index: 3 },
    count: 0,
    bounds: null,
    hash: 2166136261,
    coarse: [],
  },
  {
    name: 'clip-z-low',
    dims: [6, 10, 10],
    centre: [0.5, 5.0, 5.0],
    radius: 3.0,
    scale: null,
    plane: null,
    count: 72,
    bounds: [0, 2, 2, 7, 2, 7],
    hash: 3193864357,
    coarse: [[[2, 2, 2], 12, [0, 1, 1, 3, 1, 3], 3687468773]],
  },
  {
    name: 'clip-z-high',
    dims: [6, 10, 10],
    centre: [5.5, 5.0, 5.0],
    radius: 3.0,
    scale: null,
    plane: null,
    count: 72,
    bounds: [3, 5, 2, 7, 2, 7],
    hash: 16402725,
    coarse: [[[2, 2, 2], 6, [2, 2, 1, 3, 1, 3], 3596338197]],
  },
  {
    name: 'clip-y-low',
    dims: [6, 10, 10],
    centre: [3.0, 0.5, 5.0],
    radius: 3.0,
    scale: null,
    plane: null,
    count: 72,
    bounds: [0, 5, 0, 2, 2, 7],
    hash: 342693957,
    coarse: [[[2, 2, 2], 12, [0, 2, 0, 1, 1, 3], 2936230215]],
  },
  {
    name: 'clip-y-high',
    dims: [6, 10, 10],
    centre: [3.0, 9.5, 5.0],
    radius: 3.0,
    scale: null,
    plane: null,
    count: 72,
    bounds: [0, 5, 7, 9, 2, 7],
    hash: 2383361989,
    coarse: [[[2, 2, 2], 6, [0, 2, 4, 4, 1, 3], 2412400535]],
  },
  {
    name: 'clip-x-low',
    dims: [6, 10, 10],
    centre: [3.0, 5.0, 0.5],
    radius: 3.0,
    scale: null,
    plane: null,
    count: 72,
    bounds: [0, 5, 2, 7, 0, 2],
    hash: 2272845989,
    coarse: [[[2, 2, 2], 12, [0, 2, 1, 3, 0, 1], 3929492151]],
  },
  {
    name: 'clip-x-high',
    dims: [6, 10, 10],
    centre: [3.0, 5.0, 9.5],
    radius: 3.0,
    scale: null,
    plane: null,
    count: 72,
    bounds: [0, 5, 2, 7, 7, 9],
    hash: 1922683365,
    coarse: [[[2, 2, 2], 6, [0, 2, 1, 3, 4, 4], 3989161991]],
  },
  {
    name: 'clip-every-face',
    dims: [6, 10, 10],
    centre: [3.0, 5.0, 5.0],
    radius: 20.0,
    scale: null,
    plane: null,
    count: 600,
    bounds: [0, 5, 0, 9, 0, 9],
    hash: 3729080933,
    coarse: [[[3, 3, 3], 32, [0, 1, 0, 3, 0, 3], 1556374725]],
  },
  {
    name: 'entirely-outside',
    dims: [6, 10, 10],
    centre: [-8.0, 5.0, 5.0],
    radius: 3.0,
    scale: null,
    plane: null,
    count: 0,
    bounds: null,
    hash: 2166136261,
    coarse: [],
  },
  {
    name: 'dev-dataset-orb',
    dims: [45, 512, 512],
    centre: [22.5, 256.5, 256.5],
    radius: 40.0,
    scale: { z: 2.0, y: 0.60296875, x: 0.6029296875 },
    plane: null,
    count: 80671,
    bounds: [10, 34, 217, 295, 216, 296],
    hash: 848347560,
    coarse: [
      [[1, 2, 2], 20143, [10, 34, 109, 147, 108, 148], 1298913839],
      [[2, 4, 4], 2495, [5, 17, 55, 73, 54, 74], 1795790636],
      [[1, 3, 3], 8957, [10, 34, 73, 98, 73, 98], 2526211307],
    ],
  },
  {
    name: 'dev-dataset-disk',
    dims: [45, 512, 512],
    centre: [22.5, 256.25, 255.75],
    radius: 60.0,
    scale: { z: 2.0, y: 0.60296875, x: 0.6029296875 },
    plane: { axis: 'z', index: 22 },
    count: 11311,
    bounds: [22, 22, 196, 315, 196, 315],
    hash: 4090540587,
    coarse: [[[1, 2, 2], 2825, [22, 22, 98, 157, 98, 157], 886965554]],
  },
]

const bounds = (set: VoxelSet) => {
  const b = voxelBounds(set)
  return b === null ? null : [b.z0, b.z1, b.y0, b.y1, b.x0, b.x1]
}

const assertMatches = (
  set: VoxelSet,
  count: number,
  want: number[] | null,
  hash: number,
  what: string,
) => {
  expect(voxelCount(set), `count: ${what}`).toBe(count)
  expect(bounds(set), `bounds: ${what}`).toEqual(want)
  expect(stampHash(set), `hash: ${what}`).toBe(hash)
}

describe('level-0 stamp rasterization', () => {
  it('covers the cases the contract names', () => {
    expect(CASES.length).toBeGreaterThanOrEqual(15)
    const names = CASES.map((c) => c.name)
    for (const required of [
      'fractional-centre',
      'radius-one',
      'centre-on-voxel-boundary',
      'anisotropic',
      'plane-z',
      'plane-y',
      'plane-x',
      'clip-z-low',
      'clip-z-high',
      'clip-y-low',
      'clip-y-high',
      'clip-x-low',
      'clip-x-high',
    ]) {
      expect(names).toContain(required)
    }
    expect(
      CASES.some((c) => c.coarse.some(([factor]) => factor.some((f) => f === 3))),
      'a non-power-of-two pyramid factor',
    ).toBe(true)
  })

  for (const c of CASES) {
    it(`matches the stamp contract: ${c.name}`, () => {
      const set = stampVoxels(c.centre, c.radius, c.scale, c.plane, c.dims)
      assertMatches(set, c.count, c.bounds, c.hash, c.name)
      for (const [factor, count, want, hash] of c.coarse) {
        assertMatches(
          downsample(set, factor),
          count,
          want,
          hash,
          `${c.name} at [${factor.join(', ')}]`,
        )
      }
    })
  }
})
