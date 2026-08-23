import { describe, expect, it } from 'vitest'
import { distinguishableColors, srgbToLab, type Rgb01 } from './palette'

const WHITE: Rgb01 = [1, 1, 1]
const BLACK: Rgb01 = [0, 0, 0]

const dist2 = (a: Rgb01, b: Rgb01): number => {
  const [al, aa, ab] = srgbToLab(a)
  const [bl, ba, bb] = srgbToLab(b)
  return (al - bl) ** 2 + (aa - ba) ** 2 + (ab - bb) ** 2
}

describe('srgbToLab', () => {
  it('places the reference colours where CIE puts them', () => {
    const [l, a, b] = srgbToLab(WHITE)
    expect(l).toBeCloseTo(100, 4)
    expect(a).toBeCloseTo(0, 4)
    expect(b).toBeCloseTo(0, 4)
    expect(srgbToLab(BLACK)[0]).toBeCloseTo(0, 6)
    // mid grey sits near L* 53, the classic sRGB gamma landmark
    expect(srgbToLab([0.5, 0.5, 0.5])[0]).toBeCloseTo(53.39, 1)
  })
})

describe('distinguishableColors', () => {
  it('is a prefix sequence: asking for more never re-colours the earlier ones', () => {
    const eight = distinguishableColors(8, [WHITE, BLACK])
    const twenty = distinguishableColors(20, [WHITE, BLACK])
    expect(twenty.slice(0, 8)).toEqual(eight)
  })

  it('is deterministic, so a cell keeps its colour across sessions', () => {
    expect(distinguishableColors(12, [WHITE])).toEqual(distinguishableColors(12, [WHITE]))
  })

  it('separates every pair by more than a just-noticeable difference', () => {
    const colors = distinguishableColors(24, [WHITE, BLACK])
    for (let i = 0; i < colors.length; i++) {
      for (let j = i + 1; j < colors.length; j++) {
        // ΔE 2.3 is the classic JND; the greedy pick clears it by a wide margin
        expect(Math.sqrt(dist2(colors[i] as Rgb01, colors[j] as Rgb01))).toBeGreaterThan(2.3)
      }
    }
  })

  it('stays away from the backgrounds it was given', () => {
    const colors = distinguishableColors(16, [WHITE, BLACK])
    for (const c of colors) {
      expect(Math.sqrt(dist2(c, WHITE))).toBeGreaterThan(2.3)
      expect(Math.sqrt(dist2(c, BLACK))).toBeGreaterThan(2.3)
    }
  })

  it('honours the lightness floor, which keeps a label readable over a dark image', () => {
    for (const c of distinguishableColors(32, [WHITE, BLACK], 45)) {
      expect(srgbToLab(c)[0]).toBeGreaterThanOrEqual(45)
    }
  })

  it('refuses a request the grid cannot serve', () => {
    expect(() => distinguishableColors(4, [WHITE, BLACK], 99.9)).toThrow(/light enough/)
    expect(() => distinguishableColors(30_000, [WHITE])).toThrow(/cannot distinguish/)
    expect(() => distinguishableColors(4, [])).toThrow(/background/)
  })
})
