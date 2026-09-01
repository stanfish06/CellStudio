import { beforeEach, describe, expect, it } from 'vitest'
import { devProject } from '../test/data'
import { pendingLinkStale, useNav, type OrbitCamera } from './nav'

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

describe('pendingLink state machine.', () => {
  const project = devProject({ hasGraph: true })

  beforeEach(() => {
    useNav.getState().initProject(project)
  })

  it('arms on entering the link tool with a selection, capturing session and version', () => {
    useNav.getState().select(42)
    useNav.getState().setTool('link')
    expect(useNav.getState().tool).toBe('link')
    expect(useNav.getState().pendingLink).toEqual({
      parentId: 42,
      sessionId: 'session-1',
      graphVersion: 1,
    })
  })

  it('refuses to arm without a selection or without a graph', () => {
    useNav.getState().setTool('link')
    expect(useNav.getState().tool).toBe('pointer')
    expect(useNav.getState().pendingLink).toBe(null)

    useNav.getState().initProject(devProject({ hasGraph: false }))
    useNav.getState().select(42)
    useNav.getState().setTool('link')
    expect(useNav.getState().tool).toBe('pointer')
    expect(useNav.getState().pendingLink).toBe(null)
  })

  it('disarms on a tool switch', () => {
    useNav.getState().select(42)
    useNav.getState().setTool('link')
    useNav.getState().setTool('brush')
    expect(useNav.getState().tool).toBe('brush')
    expect(useNav.getState().pendingLink).toBe(null)
  })

  it('disarms on cancel (Esc) and leaves the inert link tool', () => {
    useNav.getState().select(42)
    useNav.getState().setTool('link')
    useNav.getState().cancelLink()
    expect(useNav.getState().pendingLink).toBe(null)
    expect(useNav.getState().tool).toBe('pointer')
    // a second cancel writes nothing
    const before = useNav.getState()
    useNav.getState().cancelLink()
    expect(useNav.getState()).toBe(before)
  })

  it('disarms and reverts to the pointer on completion', () => {
    useNav.getState().select(42)
    useNav.getState().setTool('link')
    useNav.getState().completeLink()
    expect(useNav.getState().pendingLink).toBe(null)
    expect(useNav.getState().tool).toBe('pointer')
  })

  it('clears pendingLink, selection and tool when a project replaces the current one', () => {
    useNav.getState().select(42)
    useNav.getState().setTool('link')
    useNav.getState().initProject(
      devProject({
        sessionId: 'session-2',
        versions: { sessionId: 'session-2', image: 1, labels: 1, graph: 1, settings: 1 },
      }),
    )
    expect(useNav.getState().pendingLink).toBe(null)
    expect(useNav.getState().selection).toBe(null)
    expect(useNav.getState().tool).toBe('pointer')
  })

  it('clears the armed state across a backend restart of the same project', () => {
    useNav.getState().select(42)
    useNav.getState().setTool('link')
    // same paths, new session id — what a backend restart hands initProject
    useNav.getState().initProject(
      devProject({
        hasGraph: true,
        sessionId: 'session-2',
        versions: { sessionId: 'session-2', image: 1, labels: 1, graph: 1, settings: 1 },
      }),
    )
    expect(useNav.getState().pendingLink).toBe(null)
    expect(useNav.getState().selection).toBe(null)
    expect(useNav.getState().tool).toBe('pointer')
  })

  it('flags a pendingLink as stale across sessions and graph versions', () => {
    useNav.getState().select(42)
    useNav.getState().setTool('link')
    const pending = useNav.getState().pendingLink
    if (!pending) throw new Error('expected an armed link')
    expect(pendingLinkStale(pending, project)).toBe(false)
    expect(
      pendingLinkStale(pending, {
        ...project,
        versions: { ...project.versions, graph: 2 },
      }),
    ).toBe(true)
    expect(pendingLinkStale(pending, { ...project, sessionId: 'session-2' })).toBe(true)
  })
})

describe('markGraphPresent', () => {
  it('flips hasGraph once without touching anything else', () => {
    useNav.getState().initProject(devProject({ hasGraph: false }))
    const generation = useNav.getState().generation
    useNav.getState().markGraphPresent()
    expect(useNav.getState().project?.hasGraph).toBe(true)
    expect(useNav.getState().generation).toBe(generation)
    const project = useNav.getState().project
    useNav.getState().markGraphPresent()
    expect(useNav.getState().project).toBe(project)
  })
})
