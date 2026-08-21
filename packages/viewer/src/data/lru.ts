/** Byte-bounded LRU. */
export class ByteLru<V> {
  private entries = new Map<string, { value: V; bytes: number }>()
  private used = 0

  constructor(private limit: number) {}

  get size(): number {
    return this.entries.size
  }

  get bytes(): number {
    return this.used
  }

  get capacity(): number {
    return this.limit
  }

  has(key: string): boolean {
    return this.entries.has(key)
  }

  get(key: string): V | undefined {
    const entry = this.entries.get(key)
    if (!entry) return undefined
    this.entries.delete(key)
    this.entries.set(key, entry)
    return entry.value
  }

  /** Read without reordering — for stats and tests. */
  peek(key: string): V | undefined {
    return this.entries.get(key)?.value
  }

  set(key: string, value: V, bytes: number): void {
    this.delete(key)
    this.entries.set(key, { value, bytes })
    this.used += bytes
    this.evict()
  }

  delete(key: string): boolean {
    const entry = this.entries.get(key)
    if (!entry) return false
    this.used -= entry.bytes
    this.entries.delete(key)
    return true
  }

  /** Drop every entry whose key satisfies the predicate — version-bump invalidation. */
  deleteWhere(pred: (key: string) => boolean): number {
    let dropped = 0
    for (const key of [...this.entries.keys()]) {
      if (pred(key) && this.delete(key)) dropped += 1
    }
    return dropped
  }

  keys(): string[] {
    return [...this.entries.keys()]
  }

  clear(): void {
    this.entries.clear()
    this.used = 0
  }

  resize(limit: number): void {
    this.limit = limit
    this.evict()
  }

  private evict(): void {
    for (const key of this.entries.keys()) {
      if (this.used <= this.limit) break
      this.delete(key)
    }
  }
}
