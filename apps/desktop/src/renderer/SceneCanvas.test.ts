import { describe, expect, it } from 'vitest'
import type { OrbitViewState } from '@deck.gl/core'
import { isModified, sameOrbitState } from './SceneCanvas'

const orbit = (o: Partial<OrbitViewState> = {}): OrbitViewState => ({
  target: [10, 20, 30],
  zoom: -1.5,
  rotationX: 25,
  rotationOrbit: 40,
  ...o,
})

describe('sameOrbitState', () => {
  it('holds for the pose deck echoes back unchanged', () => {
    expect(sameOrbitState(orbit(), orbit())).toBe(true)
    // A click with no drag emits panStart's anchors only, so the pose is identical.
    expect(sameOrbitState(orbit(), { ...orbit(), target: [10, 20, 30] })).toBe(true)
  })

  it('fails on any moved axis, so a real gesture is never dropped', () => {
    expect(sameOrbitState(orbit(), orbit({ rotationOrbit: 41 }))).toBe(false)
    expect(sameOrbitState(orbit(), orbit({ rotationX: 26 }))).toBe(false)
    expect(sameOrbitState(orbit(), orbit({ zoom: -1.49 }))).toBe(false)
    expect(sameOrbitState(orbit(), orbit({ target: [11, 20, 30] }))).toBe(false)
    expect(sameOrbitState(orbit(), orbit({ target: [10, 21, 30] }))).toBe(false)
    expect(sameOrbitState(orbit(), orbit({ target: [10, 20, 31] }))).toBe(false)
  })
})

describe('isModified', () => {
  it('reports every modifier deck treats as a function key', () => {
    for (const key of ['altKey', 'ctrlKey', 'metaKey', 'shiftKey']) {
      expect(isModified({ srcEvent: { [key]: true } })).toBe(true)
    }
  })

  it('reports a plain click, and tolerates an event with no source', () => {
    expect(isModified({ srcEvent: {} })).toBe(false)
    expect(isModified({})).toBe(false)
    expect(isModified(null)).toBe(false)
    expect(isModified(undefined)).toBe(false)
  })
})
