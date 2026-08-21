import { describe, expect, it } from 'vitest'
import { clampAxisScale, formatAxisScale, isPhysicalScale, parseAxisScale } from './axisScale'

describe('clampAxisScale', () => {
  it('holds multipliers inside 0.1–10', () => {
    expect(clampAxisScale(8)).toBe(8)
    expect(clampAxisScale(0.05)).toBe(0.1)
    expect(clampAxisScale(0)).toBe(0.1)
    expect(clampAxisScale(-4)).toBe(0.1)
    expect(clampAxisScale(11)).toBe(10)
    expect(clampAxisScale(1e6)).toBe(10)
  })

  it('falls back to physical scale for a non-number', () => {
    expect(clampAxisScale(Number.NaN)).toBe(1)
    expect(clampAxisScale(Number.POSITIVE_INFINITY)).toBe(1)
  })

  it('accepts the range from the caller so the nav store stays the source of truth', () => {
    expect(clampAxisScale(20, 0.1, 100)).toBe(20)
    expect(clampAxisScale(0.5, 1, 4)).toBe(1)
  })
})

describe('parseAxisScale', () => {
  it('reads numeric entry and clamps it', () => {
    expect(parseAxisScale('8')).toBe(8)
    expect(parseAxisScale(' 8.5 ')).toBe(8.5)
    expect(parseAxisScale('8×')).toBe(8)
    expect(parseAxisScale('40')).toBe(10)
    expect(parseAxisScale('0.01')).toBe(0.1)
  })

  it('rejects entry that is not a multiplier', () => {
    expect(parseAxisScale('')).toBeNull()
    expect(parseAxisScale('  ')).toBeNull()
    expect(parseAxisScale('big')).toBeNull()
  })
})

describe('isPhysicalScale', () => {
  it('is true only at the reset state', () => {
    expect(isPhysicalScale({ z: 1, y: 1, x: 1 })).toBe(true)
    expect(isPhysicalScale({ z: 8, y: 1, x: 1 })).toBe(false)
    expect(isPhysicalScale({ z: 1, y: 1, x: 0.5 })).toBe(false)
  })
})

describe('formatAxisScale', () => {
  it('reads back as a multiplier', () => {
    expect(formatAxisScale(1)).toBe('1×')
    expect(formatAxisScale(8.5)).toBe('8.5×')
    expect(formatAxisScale(0.125)).toBe('0.13×')
  })
})
