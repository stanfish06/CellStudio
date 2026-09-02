import type { PlaneBuffer } from '@cellstudio/api-client'
import { describe, expect, it } from 'vitest'
import { defaultLabelColor, labelHex, outlineImage, type OutlineImage } from './labelOutline'

// 5×5 plane: cell 7 fills a 3×3 block in the middle, cell 9 sits in the top-left corner.
const plane = (): PlaneBuffer => {
  const ids = new Uint32Array(25)
  for (let y = 1; y <= 3; y += 1) for (let x = 1; x <= 3; x += 1) ids[y * 5 + x] = 7
  ids[0] = 9
  return { shape: [5, 5], channels: 1, dtype: 'u32', level: 0, data: ids.buffer }
}

const alphaAt = (img: OutlineImage, x: number, y: number) => img.data[(y * 5 + x) * 4 + 3]
const rgbAt = (img: OutlineImage, x: number, y: number) => {
  const o = (y * 5 + x) * 4
  return [img.data[o], img.data[o + 1], img.data[o + 2]]
}

describe('outlineImage', () => {
  it('paints the ring of a highlighted cell plus one pixel outside, leaving its core clear', () => {
    const img = outlineImage(plane(), new Map([[7, [255, 0, 0]]]))
    expect(img).not.toBeNull()
    if (!img) return
    expect(alphaAt(img, 2, 2)).toBe(0)
    expect(alphaAt(img, 1, 1)).toBe(255)
    expect(rgbAt(img, 1, 1)).toEqual([255, 0, 0])
    expect(alphaAt(img, 0, 2)).toBe(255)
    expect(alphaAt(img, 4, 4)).toBe(0)
    expect(alphaAt(img, 0, 0)).toBe(0)
  })

  it("never spills one cell's colour onto another highlighted cell", () => {
    const img = outlineImage(
      plane(),
      new Map([
        [7, [255, 0, 0]],
        [9, [0, 0, 255]],
      ]),
    )
    if (!img) throw new Error('expected an image')
    expect(rgbAt(img, 0, 0)).toEqual([0, 0, 255])
    expect(rgbAt(img, 1, 1)).toEqual([255, 0, 0])
  })

  it('returns null with nothing to draw or a non-label plane', () => {
    expect(outlineImage(plane(), new Map())).toBeNull()
    expect(outlineImage(plane(), new Map([[42, [1, 2, 3]]]))).toBeNull()
    expect(outlineImage({ ...plane(), dtype: 'u16' }, new Map([[7, [1, 2, 3]]]))).toBeNull()
  })
})

describe('default label colours', () => {
  it('is stable per name and yields to a stored colour', () => {
    expect(defaultLabelColor('verified')).toBe(defaultLabelColor('verified'))
    expect(defaultLabelColor('verified')).toMatch(/^#[0-9a-f]{6}$/)
    expect(labelHex({ name: 'x', color: '#123456' })).toBe('#123456')
    expect(labelHex({ name: 'x', color: null })).toBe(defaultLabelColor('x'))
  })
})
