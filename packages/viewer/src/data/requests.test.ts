import { describe, expect, it, vi } from 'vitest'
import { RequestPool, isAbortError } from './requests'

interface Held {
  id: string
  signal: AbortSignal
  resolve: (v: string) => void
}

const heldPool = (maxConcurrent = 6) => {
  const held: Held[] = []
  const pool = new RequestPool<string>(
    (id, signal) => new Promise<string>((resolve) => held.push({ id, signal, resolve })),
    maxConcurrent,
  )
  return { pool, held }
}

describe('RequestPool', () => {
  it('shares one in-flight request between identical callers', async () => {
    const { pool, held } = heldPool()
    const a = pool.request('k', 'active')
    const b = pool.request('k', 'active')
    expect(held).toHaveLength(1)
    expect(pool.stats.deduped).toBe(1)
    held[0]?.resolve('v')
    await expect(a).resolves.toBe('v')
    await expect(b).resolves.toBe('v')
  })

  it('aborts the underlying request when its last consumer withdraws', async () => {
    const { pool, held } = heldPool()
    const controller = new AbortController()
    const promise = pool.request('k', 'active', controller.signal)
    controller.abort()
    await expect(promise).rejects.toSatisfy(isAbortError)
    expect(held[0]?.signal.aborted).toBe(true)
    expect(pool.stats.aborted).toBe(1)
  })

  it('keeps the request alive while another consumer still wants it', async () => {
    const { pool, held } = heldPool()
    const first = new AbortController()
    const rejected = pool.request('k', 'active', first.signal)
    const survivor = pool.request('k', 'active')
    first.abort()
    await expect(rejected).rejects.toSatisfy(isAbortError)
    expect(held[0]?.signal.aborted).toBe(false)
    held[0]?.resolve('v')
    await expect(survivor).resolves.toBe('v')
  })

  it('does not abort a request a prefetch is holding', async () => {
    const { pool, held } = heldPool()
    void pool.request('k', 'prefetch')
    const controller = new AbortController()
    const withdrawn = pool.request('k', 'active', controller.signal)
    controller.abort()
    await expect(withdrawn).rejects.toSatisfy(isAbortError)
    expect(held[0]?.signal.aborted).toBe(false)
  })

  it('dispatches active work before queued prefetches', async () => {
    const { pool, held } = heldPool(1)
    void pool.request('running', 'prefetch')
    void pool.request('warm-a', 'prefetch')
    void pool.request('warm-b', 'prefetch')
    void pool.request('now', 'active')
    expect(held.map((h) => h.id)).toEqual(['running'])
    expect(pool.queuedIds()).toEqual(['now', 'warm-a', 'warm-b'])
    held[0]?.resolve('v')
    await vi.waitFor(() => expect(held.map((h) => h.id)).toEqual(['running', 'now']))
  })

  it('promotes a queued prefetch that the active view now needs', () => {
    const { pool } = heldPool(1)
    void pool.request('running', 'active')
    void pool.request('later', 'prefetch')
    void pool.request('warm', 'prefetch')
    void pool.request('later', 'active')
    expect(pool.queuedIds()).toEqual(['later', 'warm'])
  })
})
