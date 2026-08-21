import type { Histogram } from '@cellstudio/api-client'
import { describe, expect, it } from 'vitest'
import { HIST_HEIGHT, HIST_WIDTH, histogramGeometry, lutTransfer } from './histogram'

const hist: Histogram = {
  counts: [0, 10, 40, 20, 5, 0, 0, 0],
  min: 0,
  max: 65535,
  sampled: true,
}

const geometry = (window: [number, number], gamma = 1, h: Histogram | null = hist) =>
  histogramGeometry({ hist: h, domain: [0, 65535], window, gamma })

/** Y of the curve at a given fraction along it, read back out of the path. */
const curvePoint = (path: string, fraction: number) => {
  const points = path
    .slice(1)
    .split('L')
    .map((p) => p.split(' ').map(Number) as [number, number])
  const point = points[Math.round(fraction * (points.length - 1))]
  if (!point) throw new Error('empty curve')
  return { x: point[0], y: point[1] }
}

describe('histogramGeometry', () => {
  it('places the shaded window and the limit lines at the display limits', () => {
    const g = geometry([0, 32767.5])
    expect(g.width).toBe(HIST_WIDTH)
    expect(g.height).toBe(HIST_HEIGHT)
    expect(g.minX).toBeCloseTo(0, 6)
    expect(g.maxX).toBeCloseTo(HIST_WIDTH / 2, 6)
    expect(g.window.x).toBeCloseTo(0, 6)
    expect(g.window.width).toBeCloseTo(HIST_WIDTH / 2, 6)
  })

  it('follows the limits as they move', () => {
    const narrow = geometry([16383.75, 49151.25])
    expect(narrow.window.x).toBeCloseTo(HIST_WIDTH * 0.25, 6)
    expect(narrow.window.width).toBeCloseTo(HIST_WIDTH * 0.5, 6)
    const wider = geometry([0, 65535])
    expect(wider.window.x).toBeCloseTo(0, 6)
    expect(wider.window.width).toBeCloseTo(HIST_WIDTH, 6)
  })

  it('keeps a positive-width window when the limits cross', () => {
    const g = geometry([49151.25, 16383.75])
    expect(g.window.x).toBeCloseTo(HIST_WIDTH * 0.25, 6)
    expect(g.window.width).toBeCloseTo(HIST_WIDTH * 0.5, 6)
    // The lines stay on their own limits even inverted, so the drag reads correctly.
    expect(g.minX).toBeCloseTo(HIST_WIDTH * 0.75, 6)
    expect(g.maxX).toBeCloseTo(HIST_WIDTH * 0.25, 6)
  })

  it('clamps limits outside the histogram domain to the plot edges', () => {
    const g = geometry([-5000, 999999])
    expect(g.minX).toBe(0)
    expect(g.maxX).toBe(HIST_WIDTH)
  })

  it('draws the distribution normalized to its peak bin', () => {
    const g = geometry([0, 65535])
    expect(g.fill.startsWith(`M0 ${HIST_HEIGHT}`)).toBe(true)
    expect(g.fill.endsWith('Z')).toBe(true)
    expect(g.outline).toContain(`${HIST_WIDTH} ${HIST_HEIGHT}`)

    const ys = g.outline
      .slice(1)
      .split('L')
      .map((p) => Number(p.split(' ')[1]))
    // Peak bin (index 2 of 8) touches the top; empty bins sit on the baseline.
    expect(ys.indexOf(0)).toBe(2)
    expect(ys[0]).toBe(HIST_HEIGHT)
    expect(ys[7]).toBe(HIST_HEIGHT)
  })

  it('leaves the distribution empty until counts arrive', () => {
    const g = geometry([0, 65535], 1, null)
    expect(g.fill).toBe('')
    expect(g.outline).toBe('')
    // Window and curve still draw over the empty plot, across the dtype domain.
    expect(g.domain).toEqual([0, 65535])
    expect(g.window.width).toBeCloseTo(HIST_WIDTH, 6)
  })

  it('spans the transfer curve across the window only', () => {
    const g = geometry([16383.75, 49151.25])
    expect(curvePoint(g.curve, 0).x).toBeCloseTo(HIST_WIDTH * 0.25, 1)
    expect(curvePoint(g.curve, 1).x).toBeCloseTo(HIST_WIDTH * 0.75, 1)
    expect(curvePoint(g.curve, 0).y).toBeCloseTo(HIST_HEIGHT - 2, 6)
    expect(curvePoint(g.curve, 1).y).toBeCloseTo(2, 6)
  })

  it('draws a straight transfer curve at gamma 1 and a bowed one otherwise', () => {
    const straight = curvePoint(geometry([0, 65535], 1).curve, 0.5).y
    expect(straight).toBeCloseTo((HIST_HEIGHT - 2 + 2) / 2, 6)

    // gamma > 1 darkens the midtones: the curve sits lower (larger y) than the diagonal.
    const dark = curvePoint(geometry([0, 65535], 2).curve, 0.5).y
    expect(dark).toBeGreaterThan(straight)

    // gamma < 1 brightens them.
    const bright = curvePoint(geometry([0, 65535], 0.2).curve, 0.5).y
    expect(bright).toBeLessThan(straight)
  })

  it('labels the axis with the domain it drew', () => {
    expect(geometry([0, 65535]).ticks).toEqual(['0', '32.8k', '65.5k'])
    const narrow = histogramGeometry({
      hist: { counts: [1, 2], min: 0, max: 400, sampled: false },
      domain: [0, 65535],
      window: [0, 400],
      gamma: 1,
    })
    expect(narrow.ticks).toEqual(['0', '200', '400'])
  })
})

describe('lutTransfer', () => {
  it('is the identity at gamma 1 and clamps outside [0,1]', () => {
    expect(lutTransfer(0.5, 1)).toBeCloseTo(0.5, 12)
    expect(lutTransfer(-1, 1)).toBe(0)
    expect(lutTransfer(4, 1)).toBe(1)
  })

  it('pins the endpoints at every gamma', () => {
    for (const gamma of [0.2, 1, 3]) {
      expect(lutTransfer(0, gamma)).toBe(0)
      expect(lutTransfer(1, gamma)).toBe(1)
    }
  })
})
