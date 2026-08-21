import { afterEach, describe, expect, it, vi } from 'vitest'
import { CursorReadout } from './cursorReadout'
import { TrackSource } from '../data/trackSource'
import { makeWorldTransform } from '../data/world'
import { FakeApi, cell } from '../test/data'

const flush = () => new Promise((r) => setTimeout(r, 0))

describe('CursorReadout', () => {
  afterEach(() => vi.useRealTimers())

  it('coalesces a burst of pointer moves into one lookup at the latest position', async () => {
    const api = new FakeApi()
    api.auto = false
    const readout = new CursorReadout({ api, throttleMs: 50, now: () => 0 })
    vi.useFakeTimers()
    for (let x = 0; x < 100; x += 1) readout.move([1, 10, x], { t: 4, channel: 1 })
    await vi.advanceTimersByTimeAsync(60)
    expect(api.pixelCalls).toHaveLength(1)
    expect(api.pixelCalls[0]).toMatchObject({ x: 99, y: 10, z: 1, t: 4, c: 1, layer: 'image' })
    expect(readout.lookupCount).toBe(1)
  })

  it('issues the next lookup for the position that arrived while one was in flight', async () => {
    const api = new FakeApi()
    api.auto = false
    let now = 0
    const readout = new CursorReadout({ api, throttleMs: 0, now: () => now })
    vi.useFakeTimers()
    readout.move([1, 10, 1], { t: 0, channel: 0 })
    await vi.advanceTimersByTimeAsync(1)
    expect(api.pixelCalls).toHaveLength(1)
    readout.move([1, 10, 2], { t: 0, channel: 0 })
    readout.move([1, 10, 3], { t: 0, channel: 0 })
    expect(api.pixelCalls).toHaveLength(1)
    now = 100
    api.settlePixel(0, 7)
    await vi.advanceTimersByTimeAsync(1)
    expect(api.pixelCalls).toHaveLength(2)
    expect(api.pixelCalls[1]?.x).toBe(3)
  })

  it('reports coordinates on the move and the value once the lookup lands', async () => {
    const api = new FakeApi()
    api.pixelValue = 4242
    const readout = new CursorReadout({ api, throttleMs: 0 })
    const seen: (number | null)[] = []
    readout.onChange(() => seen.push(readout.sample?.value ?? null))
    readout.move([2, 300, 700], { t: 9, channel: 2 })
    expect(readout.sample).toEqual({
      z: 2,
      y: 300,
      x: 700,
      value: null,
      labelId: null,
      trackId: null,
    })
    await flush()
    expect(readout.sample).toEqual({
      z: 2,
      y: 300,
      x: 700,
      value: 4242,
      labelId: null,
      trackId: null,
    })
    expect(seen).toEqual([null, 4242])
  })

  it('matches the pixel under a settled cursor within the 100 ms budget', async () => {
    vi.useFakeTimers()
    let clock = 0
    const api = new FakeApi()
    api.pixelValue = 512
    api.pixelLatencyMs = 30
    const readout = new CursorReadout({ api, now: () => clock })
    // A burst ending at rest, as a real pointer does.
    for (let x = 0; x < 40; x += 1) readout.move([1, 10, x], { t: 0, channel: 0 })
    const advance = async (ms: number) => {
      clock += ms
      await vi.advanceTimersByTimeAsync(ms)
    }
    await advance(100)
    expect(readout.sample).toMatchObject({ x: 39, value: 512 })
  })

  it('reads the label layer and resolves its track when masks are loaded', async () => {
    const api = new FakeApi()
    api.pixelValue = 900
    api.labelValue = 77
    api.cells = [cell(77, 0, [1, 10, 20], 5)]
    const tracks = new TrackSource(api)
    tracks.ensure(0, 1)
    await flush()
    const readout = new CursorReadout({ api, throttleMs: 0, tracks })
    readout.move([1, 10, 20], { t: 0, channel: 0, labels: true })
    await flush()
    expect(readout.sample).toEqual({
      z: 1,
      y: 10,
      x: 20,
      value: 900,
      labelId: 77,
      trackId: 5,
    })
    expect(api.pixelCalls.map((c) => c.layer)).toEqual(['image', 'labels'])
  })

  it('reports no label on background, and none at all without a label layer', async () => {
    const api = new FakeApi()
    api.labelValue = 0
    const readout = new CursorReadout({ api, throttleMs: 0 })
    readout.move([0, 0, 0], { t: 0, channel: 0, labels: true })
    await flush()
    expect(readout.sample?.labelId).toBe(null)

    api.pixelCalls.length = 0
    readout.move([0, 0, 1], { t: 0, channel: 0 })
    await flush()
    expect(api.pixelCalls.map((c) => c.layer)).toEqual(['image'])
  })

  it('converts a point on a slice quad to dataset pixels, ignoring display scale', async () => {
    const api = new FakeApi()
    const readout = new CursorReadout({ api, throttleMs: 0 })
    const transform = makeWorldTransform({ z: 2.0, y: 0.603, x: 0.603 }, { z: 8, y: 1, x: 1 })
    const zWorld = 2 * (2.0 / 0.603) * 8
    readout.moveOnSlice([700, zWorld], {
      orientation: 'xz',
      index: 300,
      transform,
      t: 1,
      channel: 0,
    })
    await flush()
    expect(readout.sample).toMatchObject({ z: 2, y: 300, x: 700 })
    expect(api.pixelCalls[0]).toMatchObject({ z: 2, y: 300, x: 700 })
  })

  it('clears without leaving a scheduled lookup behind', async () => {
    const api = new FakeApi()
    api.auto = false
    vi.useFakeTimers()
    const readout = new CursorReadout({ api, throttleMs: 30, now: () => 0 })
    readout.move([0, 0, 0], { t: 0, channel: 0 })
    readout.clear()
    await vi.advanceTimersByTimeAsync(100)
    expect(api.pixelCalls).toHaveLength(0)
    expect(readout.sample).toBe(null)
  })
})
