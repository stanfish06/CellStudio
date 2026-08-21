import type { JobState, ProjectInfo } from '@cellstudio/api-client'
import { formatPercent } from './format'

export type AdvisoryId = 'layout' | 'voxel-size'

export interface Advisory {
  id: AdvisoryId
  tone: 'warning' | 'info'
  title: string
  body: string
}

/**
 * Cards in the Inspect tab, not toasts: each stays until its condition is resolved.
 */
export function advisories(
  project: ProjectInfo | null,
  jobs: readonly JobState[] = [],
): Advisory[] {
  if (!project) return []
  const cards: Advisory[] = []

  if (project.layout.hostile) {
    const views = project.layout.affectedViews.map((v) => v.toUpperCase())
    const worst = Math.max(
      ...project.layout.affectedViews.map((v) => project.layout.amplification[v]),
      1,
    )
    cards.push({
      id: 'layout',
      tone: 'warning',
      title: `${views.join('/') || 'Slice'} reads may be slow`,
      body: `The source chunk layout reads ${formatAmplification(worst)} more data than each plane needs. ${rechunkStatus(jobs)}`,
    })
  }

  if (project.scale === null) {
    cards.push({
      id: 'voxel-size',
      tone: 'warning',
      title: 'Voxel size metadata missing',
      body: 'Views render isotropically. Set a voxel-size override in project settings, or stretch an axis with the display scale below.',
    })
  }

  return cards
}

export function rechunkJob(jobs: readonly JobState[]): JobState | null {
  return jobs.find((j) => j.kind === 'rechunk' && j.status === 'running') ?? null
}

export function activeJob(jobs: readonly JobState[]): JobState | null {
  return jobs.find((j) => j.status === 'running') ?? null
}

function rechunkStatus(jobs: readonly JobState[]): string {
  const job = rechunkJob(jobs)
  if (job) return `A brick working copy is ${formatPercent(job.progress)} complete.`
  return 'Re-chunking into a brick working copy would remove the amplification.'
}

function formatAmplification(factor: number): string {
  return `${Math.round(factor * 10) / 10}×`
}
