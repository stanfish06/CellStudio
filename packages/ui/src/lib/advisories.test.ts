import type { JobState, ProjectInfo } from '@cellstudio/api-client'
import { describe, expect, it } from 'vitest'
import { advisories } from './advisories'

const project = (patch: Partial<ProjectInfo> = {}): ProjectInfo => ({
  sessionId: 's1',
  sourcePath: '/data/embryo_04.zarr',
  projectPath: '/data/embryo_04.cellstudio',
  dims: { t: 400, c: 2, z: 45, y: 2048, x: 2048 },
  dtype: 'u16',
  scale: { z: 2, y: 0.35, x: 0.35 },
  levels: [],
  channels: [],
  versions: { sessionId: 's1', image: 1, labels: 0, graph: 0, settings: 0 },
  layout: { hostile: false, amplification: { xy: 1, xz: 1, yz: 1 }, affectedViews: [] },
  hasLabels: false,
  ...patch,
})

const hostile: Partial<ProjectInfo> = {
  layout: {
    hostile: true,
    amplification: { xy: 1, xz: 45, yz: 45 },
    affectedViews: ['xz', 'yz'],
  },
}

const rechunking: JobState = {
  id: 'j1',
  kind: 'rechunk',
  progress: 0.38,
  status: 'running',
  message: null,
}

describe('advisories', () => {
  it('shows nothing before a project is open, or on a healthy project', () => {
    expect(advisories(null)).toEqual([])
    expect(advisories(project())).toEqual([])
  })

  it('raises the layout advisory for a hostile chunk layout, naming the views', () => {
    const cards = advisories(project(hostile))
    expect(cards).toHaveLength(1)
    expect(cards[0]).toMatchObject({ id: 'layout', tone: 'warning' })
    expect(cards[0]?.title).toBe('XZ/YZ reads may be slow')
    expect(cards[0]?.body).toContain('45×')
  })

  it('carries the working-copy progress while re-chunking runs', () => {
    expect(advisories(project(hostile), [rechunking])[0]?.body).toContain('38% complete')
    expect(advisories(project(hostile), [])[0]?.body).toContain('Re-chunking')
  })

  it('clears the layout advisory once the layout is no longer hostile', () => {
    const adopted = project({
      layout: { hostile: false, amplification: { xy: 1, xz: 1, yz: 1 }, affectedViews: [] },
    })
    expect(advisories(adopted, [{ ...rechunking, progress: 1, status: 'done' }])).toEqual([])
  })

  it('raises the missing-voxel-size warning', () => {
    const cards = advisories(project({ scale: null }))
    expect(cards.map((c) => c.id)).toEqual(['voxel-size'])
    expect(cards[0]?.tone).toBe('warning')
  })

  it('stacks every condition that is true at once', () => {
    const cards = advisories(
      project({
        ...hostile,
        scale: null,
      }),
      [rechunking],
    )
    expect(cards.map((c) => c.id)).toEqual(['layout', 'voxel-size'])
  })
})
