import { beforeEach, describe, expect, it } from 'vitest'
import { devProject } from '../test/data'
import { useNav, type OrbitCamera } from './nav'

const pose = (overrides: Partial<OrbitCamera> = {}): OrbitCamera => ({
  rotationX: 40,
  rotationOrbit: 90,
  zoom: -1.5,
  target: [1, 2, 3],
  ...overrides,
})

describe('volume camera', () => {
  beforeEach(() => {
    useNav.getState().resetVolumeCamera()
  })

  it('starts unset so the scene can fit the volume', () => {
    expect(useNav.getState().volume.camera).toBe(null)
  })

  it('stores a pose without invalidating requests', () => {
    const before = useNav.getState().generation
    useNav.getState().setVolumeCamera(pose())
    expect(useNav.getState().volume.camera).toEqual(pose())
    expect(useNav.getState().generation).toBe(before)
  })

  it('resets to unset, and a second reset writes nothing', () => {
    useNav.getState().setVolumeCamera(pose())
    useNav.getState().resetVolumeCamera()
    expect(useNav.getState().volume.camera).toBe(null)

    const volume = useNav.getState().volume
    useNav.getState().resetVolumeCamera()
    expect(useNav.getState().volume).toBe(volume)
  })

  it('clears a moved camera when a project is opened', () => {
    useNav.getState().setVolumeCamera(pose())
    useNav.getState().initProject(devProject())
    expect(useNav.getState().volume.camera).toBe(null)
  })
})
