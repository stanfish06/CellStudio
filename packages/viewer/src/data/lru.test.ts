import { describe, expect, it } from 'vitest'
import { ByteLru } from './lru'

describe('ByteLru', () => {
  it('evicts least recently used entries to stay within its byte limit', () => {
    const lru = new ByteLru<string>(30)
    lru.set('a', 'a', 10)
    lru.set('b', 'b', 10)
    lru.set('c', 'c', 10)
    lru.get('a')
    lru.set('d', 'd', 10)
    expect(lru.keys()).toEqual(['c', 'a', 'd'])
    expect(lru.bytes).toBe(30)
  })

  it('accounts bytes when an entry is replaced', () => {
    const lru = new ByteLru<string>(100)
    lru.set('a', 'a', 40)
    lru.set('a', 'a2', 10)
    expect(lru.bytes).toBe(10)
    expect(lru.size).toBe(1)
  })

  it('drops matching keys and reclaims their bytes', () => {
    const lru = new ByteLru<string>(100)
    lru.set('image/v1', 'x', 10)
    lru.set('image/v2', 'y', 10)
    lru.set('labels/v1', 'z', 10)
    expect(lru.deleteWhere((k) => k.startsWith('image/'))).toBe(2)
    expect(lru.keys()).toEqual(['labels/v1'])
    expect(lru.bytes).toBe(10)
  })

  it('evicts on shrink', () => {
    const lru = new ByteLru<string>(100)
    lru.set('a', 'a', 40)
    lru.set('b', 'b', 40)
    lru.resize(50)
    expect(lru.keys()).toEqual(['b'])
  })
})
