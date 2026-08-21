import { describe, expect, it } from 'vitest'
import { BUDGETS_MS, PerfMonitor } from './perf'

const clock = () => {
  let now = 0
  return { now: () => now, advance: (ms: number) => (now += ms) }
}

describe('PerfMonitor spans', () => {
  it('accumulates per-kind time and bytes', () => {
    const c = clock()
    const perf = new PerfMonitor({ now: c.now })
    const network = perf.span('plane', 'network')
    c.advance(40)
    expect(network.end(1024)).toBe(40)
    const decode = perf.span('plane', 'decode')
    c.advance(5)
    decode.end()
    expect(perf.spanTotals().network).toEqual({ count: 1, totalMs: 40, bytes: 1024 })
    expect(perf.spanTotals().decode?.totalMs).toBe(5)
  })
})

describe('PerfMonitor interactions', () => {
  it('measures from input to the presented frame', () => {
    const c = clock()
    const perf = new PerfMonitor({ now: c.now })
    perf.begin('ortho-step', 'key-1')
    c.advance(70)
    expect(perf.presented('key-1')).toBe(70)
    expect(perf.stats('ortho-step')).toMatchObject({ n: 1, p50: 70, p95: 70 })
  })

  it('ignores a presentation for something it was not waiting on', () => {
    const perf = new PerfMonitor({ now: clock().now })
    expect(perf.presented('never-requested')).toBe(null)
  })

  it('drops superseded interactions instead of counting them', () => {
    const c = clock()
    const perf = new PerfMonitor({ now: c.now })
    perf.begin('ortho-step', 'stale')
    perf.begin('ortho-step', 'fresh')
    perf.cancel('stale')
    c.advance(20)
    perf.presented('fresh')
    expect(perf.pendingKeys()).toEqual([])
    expect(perf.stats('ortho-step').n).toBe(1)
  })

  it('reports p95 over the sample window', () => {
    const c = clock()
    const perf = new PerfMonitor({ now: c.now })
    for (let i = 1; i <= 100; i += 1) {
      perf.begin('xy-step', `k${i}`)
      c.advance(i)
      perf.presented(`k${i}`)
    }
    expect(perf.stats('xy-step').p95).toBe(95)
    expect(perf.stats('xy-step').max).toBe(100)
  })

  it('records budget violations and can throw in bench mode', () => {
    const c = clock()
    const perf = new PerfMonitor({ now: c.now })
    perf.begin('view-switch', 'k')
    c.advance(BUDGETS_MS['view-switch'] + 1)
    perf.presented('k')
    expect(perf.violations[0]).toMatchObject({ interaction: 'view-switch', budget: 100 })

    const strict = new PerfMonitor({ now: c.now, assertBudgets: true })
    strict.begin('t-step-3d', 'k')
    c.advance(500)
    expect(() => strict.presented('k')).toThrow(/t-step-3d p95/)
  })
})

describe('PerfMonitor frame readout', () => {
  it('averages frame time into an fps figure', () => {
    const c = clock()
    const perf = new PerfMonitor({ now: c.now })
    for (let i = 0; i < 11; i += 1) {
      perf.frame()
      c.advance(16)
    }
    const readout = perf.readout()
    expect(readout.frames).toBe(10)
    expect(readout.frameTimeMs).toBeCloseTo(16, 6)
    expect(readout.fps).toBeCloseTo(62.5, 3)
  })

  it('starts empty', () => {
    expect(new PerfMonitor().readout()).toEqual({ fps: 0, frameTimeMs: 0, frames: 0 })
  })
})
