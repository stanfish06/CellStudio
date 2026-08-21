import type { PhysicalScale } from '@cellstudio/api-client'
import { describe, expect, it } from 'vitest'
import {
  chipText,
  hudChips,
  orientationChip,
  pixelsPerUm,
  scaleBar,
  voxelChip,
  zoomPercent,
} from './hud'

const scale: PhysicalScale = { z: 2, y: 0.35, x: 0.35 }

describe('hudChips', () => {
  it('reports orientation, slice, level, zoom, frame and voxel size', () => {
    const chips = hudChips({
      activeView: 'xy',
      slice: { axis: 'z', index: 17, max: 44 },
      t: 126,
      tMax: 399,
      level: 0,
      zoom: 0,
      scale,
    })
    expect(chipText(chips.orientation)).toBe('XY · Z 17/44')
    expect(chips.level).toBe('Scale 0 · 100%')
    expect(chipText(chips.frame)).toBe('T 126/399')
    expect(chips.voxel).toBe('2.0 µm Z · 0.35 µm XY')
  })

  it('emphasizes the view name and the frame number, as the prototype does', () => {
    const chips = hudChips({
      activeView: 'xz',
      slice: { axis: 'y', index: 1048, max: 2047 },
      t: 126,
      tMax: 399,
      level: 0,
      zoom: 0,
      scale,
    })
    expect(chips.orientation).toEqual({ prefix: '', emphasis: 'XZ', suffix: ' · Y 1048/2047' })
    expect(chips.frame).toEqual({ prefix: 'T ', emphasis: '126', suffix: '/399' })
  })

  it('names the stepped axis per orientation and shows a projection in 3D', () => {
    expect(chipText(orientationChip('xz', { axis: 'y', index: 1048, max: 2047 }))).toBe(
      'XZ · Y 1048/2047',
    )
    expect(chipText(orientationChip('yz', { axis: 'x', index: 983, max: 2047 }))).toBe(
      'YZ · X 983/2047',
    )
    expect(chipText(orientationChip('3d', null))).toBe('3D')
  })

  it('tracks the pyramid level and zoom independently', () => {
    const chips = (level: number, zoom: number) =>
      hudChips({
        activeView: 'xz',
        slice: { axis: 'y', index: 3, max: 44 },
        t: 7,
        tMax: 399,
        level,
        zoom,
        scale,
      })
    expect(chips(2, -1).level).toBe('Scale 2 · 50%')
    expect(chips(0, 1).level).toBe('Scale 0 · 200%')
    expect(chipText(chips(2, -1).frame)).toBe('T 7/399')
  })
})

describe('zoomPercent', () => {
  it('reads deck.gl log2 zoom as a percentage', () => {
    expect(zoomPercent(0)).toBe('100%')
    expect(zoomPercent(2)).toBe('400%')
    expect(zoomPercent(-3)).toBe('12.5%')
  })
})

describe('voxelChip', () => {
  it('collapses equal in-plane spacing and spells out anisotropic XY', () => {
    expect(voxelChip({ z: 2, y: 0.603, x: 0.603 })).toBe('2.0 µm Z · 0.603 µm XY')
    expect(voxelChip({ z: 2, y: 0.5, x: 0.35 })).toBe('2.0 Z · 0.5 Y · 0.35 X µm')
  })

  it('says so when the dataset carries no scale metadata', () => {
    expect(voxelChip(null)).toBe('voxel size unknown')
  })
})

describe('scaleBar', () => {
  it('picks a 1/2/5 length near the target width', () => {
    expect(scaleBar(4.1, 82)).toEqual({ lengthPx: 82, label: '20 µm' })
    expect(scaleBar(pixelsPerUm(scale, 0), 82)).toEqual({ lengthPx: 57, label: '20 µm' })
    expect(scaleBar(pixelsPerUm(scale, 3), 82)).toEqual({ lengthPx: 46, label: '2.0 µm' })
    expect(scaleBar(200, 82)).toEqual({ lengthPx: 40, label: '0.2 µm' })
  })

  it('has nothing to draw without a physical scale', () => {
    expect(pixelsPerUm(null, 0)).toBeNull()
    expect(scaleBar(null)).toBeNull()
    expect(scaleBar(0)).toBeNull()
  })
})
