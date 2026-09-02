import { describe, expect, it } from 'vitest'
import { CursorReadout } from './cursorReadout'
import { makeWorldTransform } from '../data/world'

describe('CursorReadout', () => {
  it('reports the floored voxel on every move and notifies once per change', () => {
    const readout = new CursorReadout()
    const seen: (string | null)[] = []
    readout.onChange(() => {
      const s = readout.sample
      seen.push(s ? `${s.z},${s.y},${s.x}` : null)
    })
    readout.move([2.9, 300.2, 700.7])
    expect(readout.sample).toEqual({ z: 2, y: 300, x: 700 })
    readout.move([2.1, 300.9, 700.1])
    expect(seen).toEqual(['2,300,700'])
    readout.move([2, 301, 700])
    expect(seen).toEqual(['2,300,700', '2,301,700'])
  })

  it('converts a point on a slice quad to dataset pixels, ignoring display scale', () => {
    const readout = new CursorReadout()
    const transform = makeWorldTransform({ z: 2.0, y: 0.603, x: 0.603 }, { z: 8, y: 1, x: 1 })
    const zWorld = 2 * (2.0 / 0.603) * 8
    readout.moveOnSlice([700, zWorld], { orientation: 'xz', index: 300, transform })
    expect(readout.sample).toEqual({ z: 2, y: 300, x: 700 })
  })

  it('clears to no sample and stays quiet when already clear', () => {
    const readout = new CursorReadout()
    let changes = 0
    readout.onChange(() => (changes += 1))
    readout.move([1, 1, 1])
    readout.clear()
    readout.clear()
    expect(readout.sample).toBe(null)
    expect(changes).toBe(2)
  })
})
