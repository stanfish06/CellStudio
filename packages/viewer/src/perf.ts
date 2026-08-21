export type SpanKind = 'network' | 'decode' | 'upload' | 'presented'

export interface Span {
  end(bytes?: number): number
}

/** What the caches and scenes need from perf; `PerfMonitor` is the implementation. */
export interface PerfSink {
  span(name: string, kind: SpanKind): Span
}

export type Interaction =
  'xy-step' | 'ortho-step' | 't-step-3d' | 'view-switch' | 'first-pixels' | 'first-volume' | 'jump'

/** Design budgets, p95, measured at the presented frame. */
export const BUDGETS_MS: Record<Interaction, number> = {
  'xy-step': 100,
  'ortho-step': 100,
  't-step-3d': 200,
  'view-switch': 100,
  'first-pixels': 2000,
  'first-volume': 3000,
  jump: 500,
}

export interface SpanTotal {
  count: number
  totalMs: number
  bytes: number
}

export interface Stats {
  n: number
  p50: number
  p95: number
  max: number
}

export interface FrameReadout {
  fps: number
  frameTimeMs: number
  frames: number
}

export interface BudgetViolation {
  interaction: Interaction
  p95: number
  budget: number
}

const percentile = (sorted: number[], p: number): number => {
  if (sorted.length === 0) return 0
  const at = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1))
  return sorted[at] as number
}

export interface PerfOptions {
  now?: () => number
  /** Throw on the first over-budget interaction — benches only. */
  assertBudgets?: boolean
  /** Samples retained per interaction. */
  window?: number
  /** Frames retained for the fps readout. */
  frameWindow?: number
}

/**
 * Per-request spans (network / decode / upload) plus interaction latencies closed at the
 * presented frame, since that is where the budgets are stated.
 */
export class PerfMonitor implements PerfSink {
  private readonly now: () => number
  private readonly window: number
  private readonly frameWindow: number
  private readonly assertMode: boolean
  private spans = new Map<SpanKind, SpanTotal>()
  private samples = new Map<Interaction, number[]>()
  private pending = new Map<string, { interaction: Interaction; start: number }>()
  private frameTimes: number[] = []
  private lastFrame: number | null = null
  readonly violations: BudgetViolation[] = []

  constructor(opts: PerfOptions = {}) {
    this.now = opts.now ?? (() => performance.now())
    this.window = opts.window ?? 200
    this.frameWindow = opts.frameWindow ?? 60
    this.assertMode = opts.assertBudgets ?? false
  }

  span(name: string, kind: SpanKind): Span {
    const start = this.now()
    return {
      end: (bytes = 0) => {
        const ms = this.now() - start
        const total = this.spans.get(kind) ?? { count: 0, totalMs: 0, bytes: 0 }
        total.count += 1
        total.totalMs += ms
        total.bytes += bytes
        this.spans.set(kind, total)
        return ms
      },
    }
  }

  spanTotals(): Record<string, SpanTotal> {
    return Object.fromEntries(this.spans)
  }

  /** Opens an interaction keyed by the data it is waiting for. */
  begin(interaction: Interaction, key: string): void {
    this.pending.set(key, { interaction, start: this.now() })
  }

  /** Closes the interaction whose data just reached the screen. */
  presented(key: string): number | null {
    const open = this.pending.get(key)
    if (!open) return null
    this.pending.delete(key)
    const ms = this.now() - open.start
    const list = this.samples.get(open.interaction) ?? []
    list.push(ms)
    if (list.length > this.window) list.shift()
    this.samples.set(open.interaction, list)
    const budget = BUDGETS_MS[open.interaction]
    const p95 = this.stats(open.interaction).p95
    if (p95 > budget) {
      const violation = { interaction: open.interaction, p95, budget }
      this.violations.push(violation)
      if (this.assertMode) {
        throw new Error(
          `${violation.interaction} p95 ${p95.toFixed(1)} ms over budget ${budget} ms`,
        )
      }
    }
    return ms
  }

  /** Drops interactions superseded before they ever presented. */
  cancel(key: string): void {
    this.pending.delete(key)
  }

  pendingKeys(): string[] {
    return [...this.pending.keys()]
  }

  /** One rendered frame; feeds the status-bar fps/frame-time readout. */
  frame(): void {
    const at = this.now()
    if (this.lastFrame !== null) {
      this.frameTimes.push(at - this.lastFrame)
      if (this.frameTimes.length > this.frameWindow) this.frameTimes.shift()
    }
    this.lastFrame = at
  }

  readout(): FrameReadout {
    const n = this.frameTimes.length
    if (n === 0) return { fps: 0, frameTimeMs: 0, frames: 0 }
    const mean = this.frameTimes.reduce((a, b) => a + b, 0) / n
    return { fps: mean > 0 ? 1000 / mean : 0, frameTimeMs: mean, frames: n }
  }

  stats(interaction: Interaction): Stats {
    const list = [...(this.samples.get(interaction) ?? [])].sort((a, b) => a - b)
    return {
      n: list.length,
      p50: percentile(list, 50),
      p95: percentile(list, 95),
      max: list.length > 0 ? (list[list.length - 1] as number) : 0,
    }
  }

  reset(): void {
    this.spans.clear()
    this.samples.clear()
    this.pending.clear()
    this.frameTimes = []
    this.lastFrame = null
    this.violations.length = 0
  }
}
