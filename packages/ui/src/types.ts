import type { CSSProperties } from 'react'

export type BackendState = 'starting' | 'ready' | 'down' | 'fatal'

/** Pixel under the cursor, in dataset coordinates, never display-scaled. */
export interface CursorSample {
  z: number
  y: number
  x: number
  /** Active-channel intensity; null while the readout is in flight. */
  value: number | null
  labelId: number | null
  trackId: number | null
}

export interface PerfSample {
  fps: number
  frameMs: number
}

/** What the active scene is presenting, for the HUD chips. */
export interface DisplayState {
  level: number
  /** deck.gl log2 zoom of the active view. */
  zoom: number
}

export interface ProjectStatus {
  saved: boolean
  pendingWrites: number
}

/** Edit-journal row (`GET /edits`); the History tab is inert until editing ships. */
export interface HistoryEntry {
  seq: number
  domain: 'mask' | 'track'
  summary: string
  scope: string
  time: string
  undone: boolean
}

export type MenuId = 'file' | 'edit' | 'view' | 'help'

export const cssVars = (vars: Record<string, string | number>): CSSProperties =>
  vars as CSSProperties
