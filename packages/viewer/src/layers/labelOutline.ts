import type { PlaneBuffer } from '@cellstudio/api-client'
import type { Rgb } from './tracks'

/**
 * An RGBA image the size of a label plane with the boundary of every cell in `colors`
 * painted in its colour and everything else transparent. A boundary pixel is one whose
 * 4-neighbourhood holds a different id; the differing neighbour outside the cell is painted
 * too, so the line is two pixels wide and stays visible over the mask fill.
 */
export interface OutlineImage {
  width: number
  height: number
  /** RGBA, row-major, `width * height * 4` bytes; wrap in `ImageData` for the GPU. */
  data: Uint8ClampedArray<ArrayBuffer>
}

/** Eight well-separated hues a new definition draws from by name, until the user picks. */
export const DEFAULT_LABEL_COLORS: readonly string[] = [
  '#ffbf69',
  '#5ba7ff',
  '#52df83',
  '#ff5c73',
  '#d67cff',
  '#4be0d3',
  '#ffe14d',
  '#ff9f43',
]

export function defaultLabelColor(name: string): string {
  let hash = 0
  for (let i = 0; i < name.length; i += 1) hash = (hash * 31 + name.charCodeAt(i)) >>> 0
  return DEFAULT_LABEL_COLORS[hash % DEFAULT_LABEL_COLORS.length] as string
}

/** The colour a definition draws with: its own, or the name-derived default. */
export const labelHex = (def: { name: string; color?: string | null }): string =>
  def.color ?? defaultLabelColor(def.name)

export function outlineImage(
  plane: PlaneBuffer,
  colors: ReadonlyMap<number, Rgb>,
): OutlineImage | null {
  if (colors.size === 0 || plane.dtype !== 'u32') return null
  const [height, width] = plane.shape
  const ids = new Uint32Array(plane.data)
  if (ids.length < width * height) return null
  const out = new Uint8ClampedArray(new ArrayBuffer(width * height * 4))
  let painted = false
  const paint = (index: number, rgb: Rgb) => {
    const o = index * 4
    out[o] = rgb[0]
    out[o + 1] = rgb[1]
    out[o + 2] = rgb[2]
    out[o + 3] = 255
    painted = true
  }
  for (let y = 0; y < height; y += 1) {
    const row = y * width
    for (let x = 0; x < width; x += 1) {
      const i = row + x
      const id = ids[i] as number
      const rgb = colors.get(id)
      if (rgb === undefined) continue
      const neighbours = [
        x > 0 ? i - 1 : -1,
        x < width - 1 ? i + 1 : -1,
        y > 0 ? i - width : -1,
        y < height - 1 ? i + width : -1,
      ]
      for (const n of neighbours) {
        if (n < 0) continue
        if (ids[n] !== id) {
          paint(i, rgb)
          // only spill onto pixels no highlighted cell owns, so two neighbours keep their own line
          if (!colors.has(ids[n] as number)) paint(n, rgb)
        }
      }
    }
  }
  return painted ? { width, height, data: out } : null
}
