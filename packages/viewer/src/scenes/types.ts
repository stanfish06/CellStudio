import type { Level } from '@cellstudio/api-client'
import type { ChannelState, NavState } from '../state/nav'

/** The slice of nav state the scenes read; `useNav.getState()` satisfies it. */
export type NavSnapshot = Pick<
  NavState,
  | 'project'
  | 't'
  | 'activeView'
  | 'slices'
  | 'volume'
  | 'channels'
  | 'activeChannel'
  | 'overlays'
  | 'brush'
  | 'tool'
  | 'axisScale'
  | 'selection'
  | 'selectedLink'
  | 'pendingLink'
  | 'generation'
>

/** Read-only view state the ui needs but the frozen nav store has no field for. */
export interface SceneStatus {
  /** Matches `DisplayState` in `@cellstudio/ui`: the level on screen and deck's log2 zoom. */
  display: { level: number; zoom: number }
  /** A fetch for the current nav key is still outstanding — playback skips a tick. */
  awaitingFrame: boolean
}

export const sameStatus = (a: SceneStatus, b: SceneStatus): boolean =>
  a.awaitingFrame === b.awaitingFrame &&
  a.display.level === b.display.level &&
  a.display.zoom === b.display.zoom

export interface VisibleChannel {
  index: number
  state: ChannelState
}

export const visibleChannels = (channels: readonly ChannelState[]): VisibleChannel[] =>
  channels.map((state, index) => ({ index, state })).filter(({ state }) => state.visible)

/** Pyramid level for a deck.gl zoom: one level per halving, clamped to what exists. */
export const levelForZoom = (zoom: number, levels: readonly Level[]): number => {
  const max = Math.max(0, levels.length - 1)
  return Math.min(max, Math.max(0, Math.round(-zoom)))
}

export class Emitter {
  private listeners = new Set<() => void>()

  on(cb: () => void): () => void {
    this.listeners.add(cb)
    return () => this.listeners.delete(cb)
  }

  emit(): void {
    for (const cb of this.listeners) cb()
  }

  clear(): void {
    this.listeners.clear()
  }
}
