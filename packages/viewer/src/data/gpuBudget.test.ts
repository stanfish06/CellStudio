import { describe, expect, it } from 'vitest'
import { GPU_BYTES_PER_SAMPLE, GpuBudget } from './gpuBudget'
import { devProject } from '../test/data'

const levels = devProject().levels

describe('GpuBudget', () => {
  it('picks the finest level whose timepoint fits the volume ceiling', () => {
    const budget = new GpuBudget({ totalBytes: 1024 ** 3, volumeCeilingBytes: 8 * 1024 * 1024 })
    // level 0: 3*1024*1024 voxels * 4 B * 2 channels = 24 MB; level 1 is 6 MB.
    const plan = budget.planVolume(levels, 2, 'u16')
    expect(plan.level).toBe(1)
    expect(plan.gpuBytes).toBe(3 * 512 * 512 * GPU_BYTES_PER_SAMPLE * 2)
    expect(plan.rawBytes).toBe(3 * 512 * 512 * 2 * 2)
    expect(plan.overBudget).toBe(false)
  })

  it('falls back to the coarsest level and says it is over budget', () => {
    const budget = new GpuBudget({ totalBytes: 1024 ** 3, volumeCeilingBytes: 1024 })
    const plan = budget.planVolume(levels, 3, 'u16')
    expect(plan.level).toBe(2)
    expect(plan.overBudget).toBe(true)
  })

  it('leaves headroom for a prefetched neighbouring timepoint', () => {
    const budget = new GpuBudget({ totalBytes: 1024 ** 3, volumeCeilingBytes: 128 * 1024 * 1024 })
    const plan = budget.planVolume(levels, 1, 'u16')
    expect(plan.residentTimepoints).toBeGreaterThanOrEqual(2)
  })

  it('shrinks the tile cache by what the volume reserved', () => {
    const budget = new GpuBudget({ totalBytes: 256 * 1024 * 1024 })
    const before = budget.tileCacheSize(1024, 2)
    budget.planVolume(levels, 2, 'u16')
    const after = budget.tileCacheSize(1024, 2)
    expect(budget.volumeBytes).toBeGreaterThan(0)
    expect(after).toBeLessThan(before)
    expect(after).toBeGreaterThanOrEqual(16)
  })

  it('gives the tile cache everything back when the volume is released', () => {
    const budget = new GpuBudget({ totalBytes: 256 * 1024 * 1024 })
    budget.planVolume(levels, 2, 'u16')
    budget.releaseVolume()
    expect(budget.tileBytes).toBe(256 * 1024 * 1024)
  })

  it('never reserves more than the total', () => {
    const budget = new GpuBudget({ totalBytes: 4 * 1024 * 1024 })
    expect(budget.reserveVolume(64 * 1024 * 1024)).toBe(4 * 1024 * 1024)
    expect(budget.tileBytes).toBe(0)
  })
})
