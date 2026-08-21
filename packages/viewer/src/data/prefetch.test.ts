import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  LatestWins,
  TSettleWarmer,
  brickStart,
  nextBrickIndex,
  orthoPrefetchIndices,
} from './prefetch'
import { isAbortError } from './requests'
import type { PlaneKey, VolumeKey } from './keys'

describe('brick-granular prefetch', () => {
  it('finds the brick containing an index', () => {
    expect(brickStart(0, 16)).toBe(0)
    expect(brickStart(15, 16)).toBe(0)
    expect(brickStart(16, 16)).toBe(16)
    expect(brickStart(40, 16)).toBe(32)
  })

  it('warms one plane per brick along the scrub direction, not per plane', () => {
    // z bricks of 16 on a 45-plane stack: stepping forward inside brick 0 targets 16.
    expect(nextBrickIndex(3, 1, 16, 44)).toBe(16)
    expect(nextBrickIndex(15, 1, 16, 44)).toBe(16)
    expect(nextBrickIndex(16, 1, 16, 44)).toBe(32)
    expect(nextBrickIndex(20, -1, 16, 44)).toBe(15)
    expect(nextBrickIndex(40, 1, 16, 44)).toBe(null)
    expect(nextBrickIndex(3, -1, 16, 44)).toBe(null)
  })

  it('is a no-op on a thin stack that fits in one brick', () => {
    expect(nextBrickIndex(1, 1, 16, 2)).toBe(null)
    expect(nextBrickIndex(1, -1, 16, 2)).toBe(null)
  })

  it('ortho prefetch takes the adjacent plane plus the next brick', () => {
    expect(orthoPrefetchIndices(500, 1, 256, 1023)).toEqual([501, 512])
    expect(orthoPrefetchIndices(500, -1, 256, 1023)).toEqual([499, 255])
    expect(orthoPrefetchIndices(1023, 1, 256, 1023)).toEqual([])
  })
})

describe('LatestWins', () => {
  it('discards a stale result and keeps the newest', async () => {
    const gate = new LatestWins()
    let releaseFirst: (v: string) => void = () => {}
    const first = gate.run('gen1', () => new Promise<string>((r) => (releaseFirst = r)))
    const second = gate.run('gen2', () => Promise.resolve('newest'))
    releaseFirst('stale')
    expect(await first).toBe(null)
    expect(await second).toBe('newest')
  })

  it('aborts the superseded request', async () => {
    const gate = new LatestWins()
    let seen: AbortSignal | null = null
    const first = gate.run('gen1', (signal) => {
      seen = signal
      return new Promise<string>((_, reject) => {
        signal.addEventListener('abort', () => reject(new DOMException('x', 'AbortError')))
      })
    })
    void gate.run('gen2', () => Promise.resolve('newest'))
    expect(seen).not.toBeNull()
    expect((seen as unknown as AbortSignal).aborted).toBe(true)
    expect(await first).toBe(null)
  })

  it('propagates real failures', async () => {
    const gate = new LatestWins()
    await expect(gate.run('k', () => Promise.reject(new Error('boom')))).rejects.toThrow('boom')
    expect(isAbortError(new Error('boom'))).toBe(false)
  })
})

describe('TSettleWarmer', () => {
  afterEach(() => vi.useRealTimers())

  const key = (t: number): PlaneKey => ({
    layer: 'image',
    axis: 'xz',
    level: 0,
    t,
    c: [0],
    index: 512,
    version: 1,
  })
  const vkey = (t: number): VolumeKey => ({ layer: 'image', level: 2, t, c: 0, version: 1 })

  it('warms once after t settles, for the last plan only', () => {
    vi.useFakeTimers()
    const planes: number[] = []
    const volumes: number[] = []
    const warmer = new TSettleWarmer(
      { plane: (k) => planes.push(k.t), volume: (k) => volumes.push(k.t) },
      150,
    )
    for (let t = 1; t <= 10; t += 1) {
      warmer.schedule({ planes: [key(t)], volumes: [vkey(t)] })
      vi.advanceTimersByTime(20)
    }
    expect(planes).toEqual([])
    expect(warmer.pending).toBe(true)
    vi.advanceTimersByTime(150)
    expect(planes).toEqual([10])
    expect(volumes).toEqual([10])
    expect(warmer.pending).toBe(false)
  })

  it('cancels a pending warm', () => {
    vi.useFakeTimers()
    const planes: number[] = []
    const warmer = new TSettleWarmer({ plane: (k) => planes.push(k.t), volume: () => {} }, 150)
    warmer.schedule({ planes: [key(3)], volumes: [] })
    warmer.cancel()
    vi.advanceTimersByTime(500)
    expect(planes).toEqual([])
  })
})
