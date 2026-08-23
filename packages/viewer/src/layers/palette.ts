export type Rgb01 = readonly [number, number, number]

/** Grid divisions per RGB axis; 30³ = 27,000 candidates, as in the original. */
const GRID = 30

const linearize = (c: number): number => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)

// D65, sRGB's own white point. MATLAB's `makecform('srgb2lab')` adapts to the ICC D50
// connection space instead, which shifts the Lab values slightly and so can pick a
// different candidate at a near-tie; the spacing the algorithm produces is unaffected.
const WHITE_D65 = [0.95047, 1, 1.08883] as const

const fLab = (t: number): number =>
  t > (6 / 29) ** 3 ? Math.cbrt(t) : t / (3 * (6 / 29) ** 2) + 4 / 29

export function srgbToLab([r, g, b]: Rgb01): [number, number, number] {
  const [lr, lg, lb] = [linearize(r), linearize(g), linearize(b)]
  const x = (0.4124564 * lr + 0.3575761 * lg + 0.1804375 * lb) / WHITE_D65[0]
  const y = (0.2126729 * lr + 0.7151522 * lg + 0.072175 * lb) / WHITE_D65[1]
  const z = (0.0193339 * lr + 0.119192 * lg + 0.9503041 * lb) / WHITE_D65[2]
  const [fx, fy, fz] = [fLab(x), fLab(y), fLab(z)]
  return [116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)]
}

/**
 * `n` colours, each as far from the others and from every `background` as the grid allows.
 * Deterministic: no randomness, and ties resolve to the first candidate, so the same `n`
 * always yields the same sequence.
 */
export function distinguishableColors(
  n: number,
  background: readonly Rgb01[],
  /** Drops candidates darker than this Lab lightness; 0 keeps the original's full grid. */
  minLightness = 0,
): Rgb01[] {
  const candidates = GRID ** 3
  if (n > candidates / 3) throw new Error(`cannot distinguish ${n} colors`)
  if (background.length === 0) throw new Error('at least one background color is required')

  const rgb: Rgb01[] = []
  const labValues: number[] = []
  for (let i = 0; i < GRID; i++) {
    for (let j = 0; j < GRID; j++) {
      for (let k = 0; k < GRID; k++) {
        const colour: Rgb01 = [i / (GRID - 1), j / (GRID - 1), k / (GRID - 1)]
        const [l, a, b] = srgbToLab(colour)
        if (l < minLightness) continue
        rgb.push(colour)
        labValues.push(l, a, b)
      }
    }
  }
  const kept = rgb.length
  if (n > kept) throw new Error(`only ${kept} candidates are light enough for ${n} colors`)
  const lab = Float64Array.from(labValues)

  const bgLab = background.map(srgbToLab)
  const minDist2 = new Float64Array(kept).fill(Number.POSITIVE_INFINITY)
  // every background but the last folds in here; the last seeds the first round below
  for (let i = 0; i < bgLab.length - 1; i++) fold(lab, minDist2, bgLab[i] as number[])

  const chosen: Rgb01[] = []
  let last = bgLab[bgLab.length - 1] as number[]
  for (let i = 0; i < n; i++) {
    fold(lab, minDist2, last)
    let best = 0
    for (let c = 1; c < kept; c++) {
      if ((minDist2[c] as number) > (minDist2[best] as number)) best = c
    }
    chosen.push(rgb[best] as Rgb01)
    last = [lab[best * 3] as number, lab[best * 3 + 1] as number, lab[best * 3 + 2] as number]
  }
  return chosen
}

/** Keeps, per candidate, the squared Lab distance to the nearest colour chosen so far. */
function fold(lab: Float64Array, minDist2: Float64Array, to: number[]): void {
  const [tl, ta, tb] = to as [number, number, number]
  for (let c = 0; c < minDist2.length; c++) {
    const dl = (lab[c * 3] as number) - tl
    const da = (lab[c * 3 + 1] as number) - ta
    const db = (lab[c * 3 + 2] as number) - tb
    const d2 = dl * dl + da * da + db * db
    if (d2 < (minDist2[c] as number)) minDist2[c] = d2
  }
}
