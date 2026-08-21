/** Active-view work outranks prefetch; identical in-flight work is shared, not repeated. */
export type Priority = 'active' | 'prefetch'

export const abortError = (): Error => {
  const e = new Error('Aborted')
  e.name = 'AbortError'
  return e
}

export const isAbortError = (e: unknown): boolean =>
  e instanceof Error && (e.name === 'AbortError' || e.message === 'Aborted')

interface Entry<V> {
  id: string
  priority: Priority
  /** Consumers that can withdraw. Prefetch takes a hold instead, so it never aborts. */
  consumers: number
  holds: number
  controller: AbortController
  promise: Promise<V>
  run: () => void
  settled: boolean
  started: boolean
}

export interface PoolStats {
  running: number
  queued: number
  inflight: number
  /** Requests satisfied by joining an identical in-flight one. */
  deduped: number
  started: number
  aborted: number
}

/**
 * De-duplicates in-flight requests by id, dispatches active before prefetch under a
 * concurrency cap, and aborts a request once its last withdrawable consumer leaves.
 */
export class RequestPool<V> {
  private entries = new Map<string, Entry<V>>()
  private queue: Entry<V>[] = []
  private running = 0
  private counters = { deduped: 0, started: 0, aborted: 0 }

  constructor(
    private readonly fetcher: (id: string, signal: AbortSignal) => Promise<V>,
    private readonly maxConcurrent = 6,
  ) {}

  get stats(): PoolStats {
    return {
      running: this.running,
      queued: this.queue.length,
      inflight: this.entries.size,
      ...this.counters,
    }
  }

  inflight(id: string): boolean {
    return this.entries.has(id)
  }

  /** Ids awaiting dispatch, in the order they will be dispatched. */
  queuedIds(): string[] {
    return [
      ...this.queue.filter((e) => e.priority === 'active'),
      ...this.queue.filter((e) => e.priority === 'prefetch'),
    ].map((e) => e.id)
  }

  request(id: string, priority: Priority, signal?: AbortSignal): Promise<V> {
    const existing = this.entries.get(id)
    if (existing) this.counters.deduped += 1
    const entry = existing ?? this.create(id, priority)
    if (entry.priority === 'prefetch' && priority === 'active') this.promote(entry)
    if (priority === 'prefetch') {
      entry.holds += 1
      return entry.promise
    }
    entry.consumers += 1
    return this.withdrawable(entry, signal)
  }

  abortAll(): void {
    for (const entry of [...this.entries.values()]) this.abort(entry)
  }

  private create(id: string, priority: Priority): Entry<V> {
    const controller = new AbortController()
    let resolve: (v: V) => void = () => {}
    let reject: (e: unknown) => void = () => {}
    const promise = new Promise<V>((res, rej) => {
      resolve = res
      reject = rej
    })
    const entry: Entry<V> = {
      id,
      priority,
      consumers: 0,
      holds: 0,
      controller,
      promise,
      settled: false,
      started: false,
      run: () => {
        entry.started = true
        this.counters.started += 1
        this.running += 1
        this.fetcher(id, controller.signal).then(
          (v) => {
            this.finish(entry)
            resolve(v)
          },
          (e) => {
            this.finish(entry)
            reject(e)
          },
        )
      },
    }
    this.entries.set(id, entry)
    this.queue.push(entry)
    this.pump()
    return entry
  }

  private withdrawable(entry: Entry<V>, signal?: AbortSignal): Promise<V> {
    if (!signal) return entry.promise
    if (signal.aborted) {
      this.release(entry)
      return Promise.reject(abortError())
    }
    return new Promise<V>((resolve, reject) => {
      const onAbort = () => {
        this.release(entry)
        reject(abortError())
      }
      signal.addEventListener('abort', onAbort, { once: true })
      entry.promise.then(
        (v) => {
          signal.removeEventListener('abort', onAbort)
          resolve(v)
        },
        (e) => {
          signal.removeEventListener('abort', onAbort)
          reject(e)
        },
      )
    })
  }

  private release(entry: Entry<V>): void {
    if (entry.settled) return
    entry.consumers = Math.max(0, entry.consumers - 1)
    if (entry.consumers === 0 && entry.holds === 0) this.abort(entry)
  }

  private abort(entry: Entry<V>): void {
    if (entry.settled) return
    this.counters.aborted += 1
    entry.controller.abort()
    if (!entry.started) {
      this.queue = this.queue.filter((e) => e !== entry)
      this.entries.delete(entry.id)
      entry.settled = true
      this.pump()
    }
  }

  private promote(entry: Entry<V>): void {
    entry.priority = 'active'
    const at = this.queue.indexOf(entry)
    if (at < 0) return
    this.queue.splice(at, 1)
    this.queue.unshift(entry)
  }

  private finish(entry: Entry<V>): void {
    if (entry.settled) return
    entry.settled = true
    this.entries.delete(entry.id)
    this.running -= 1
    this.pump()
  }

  private pump(): void {
    while (this.running < this.maxConcurrent && this.queue.length > 0) {
      const at = this.queue.findIndex((e) => e.priority === 'active')
      const [next] = this.queue.splice(at >= 0 ? at : 0, 1)
      if (!next || next.settled) continue
      next.run()
    }
  }
}
