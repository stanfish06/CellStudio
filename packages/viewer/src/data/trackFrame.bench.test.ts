import { describe, expect, it } from 'vitest'
import { remapToTracks, type TrackFrame } from './trackFrame'

/**
 * Remap-pass timings for docs/bench.md (task 4.4). Numbers, not assertions — run with
 * `BENCH=1 pnpm --filter @cellstudio/viewer test -- trackFrame.bench`.
 */
const bench = process.env.BENCH ? describe : describe.skip

/** Deterministic LCG so the layout is identical across runs. */
const lcg = (seed: number) => () => (seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32

/** ~`count` round cells stamped into a zeroed volume — coherent runs, like real labels. */
function paintCells(
  data: Uint32Array,
  width: number,
  height: number,
  depth: number,
  count: number,
): number[] {
  const rand = lcg(42)
  const ids: number[] = []
  for (let i = 0; i < count; i += 1) {
    const id = i + 1
    ids.push(id)
    const cx = Math.floor(rand() * width)
    const cy = Math.floor(rand() * height)
    const cz = Math.floor(rand() * depth)
    const r = 4 + Math.floor(rand() * 8)
    const rz = Math.max(1, Math.floor(r / 4))
    for (let z = Math.max(0, cz - rz); z <= Math.min(depth - 1, cz + rz); z += 1) {
      for (let y = Math.max(0, cy - r); y <= Math.min(height - 1, cy + r); y += 1) {
        const half = Math.floor(Math.sqrt(Math.max(0, r * r - (y - cy) ** 2)))
        const row = (z * height + y) * width
        data.fill(id, row + Math.max(0, cx - half), row + Math.min(width - 1, cx + half) + 1)
      }
    }
  }
  return ids
}

const frameOf = (ids: number[]): TrackFrame => {
  const map = new Map<number, number>(ids.map((id) => [id, (id * 7919) % 0xffffff]))
  return {
    graphVersion: 1,
    revision: 1,
    t0: 0,
    t1: 10,
    ready: true,
    trackIdFor: (id) => map.get(id) ?? null,
  }
}

function measure(label: string, width: number, height: number, depth: number, cells: number): void {
  const data = new Uint32Array(width * height * depth)
  const ids = paintCells(data, width, height, depth, cells)
  const frame = frameOf(ids)
  const out = new ArrayBuffer(data.length * 4)
  const time = (reuse: boolean): number => {
    const samples: number[] = []
    for (let i = 0; i < 7; i += 1) {
      const start = performance.now()
      remapToTracks(data.buffer, 'u32', frame, reuse ? out : undefined)
      samples.push(performance.now() - start)
    }
    samples.sort((a, b) => a - b)
    return samples[Math.floor(samples.length / 2)] as number
  }
  const alloc = time(false)
  const reused = time(true)
  const mb = (data.length * 4) / 1024 / 1024
  console.log(
    `${label}: ${width}×${height}×${depth} u32 (${mb.toFixed(1)} MiB, ${cells} cells) — ` +
      `alloc ${alloc.toFixed(1)} ms, buffer-reuse ${reused.toFixed(1)} ms`,
  )
  expect(reused).toBeGreaterThan(0)
}

bench('remap pass benchmark', () => {
  it('times the plane and volume passes', () => {
    measure('plane', 1024, 1024, 1, 600)
    measure('dev proxy volume (L2)', 256, 256, 3, 600)
    measure('bench-fixture proxy volume (L2)', 512, 512, 45, 600)
    measure('worst-case volume', 1024, 1024, 45, 600)
  })
})
