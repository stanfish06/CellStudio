import type { Dtype, Histogram } from '@cellstudio/api-client'
import type { ReactNode } from 'react'
import type { DisplayState } from '../types'
import { ChannelBar } from './ChannelBar'
import { Stage } from './Stage'
import { Transport } from './Transport'
import { usePlayback } from './usePlayback'

export interface ViewPanelProps {
  scene?: ReactNode
  display: DisplayState
  histogram: Histogram | null
  dtype: Dtype | null
  /** True while the active view still owes a frame; playback holds instead of queueing. */
  awaitingFrame: boolean
  settingsOpen: boolean
  onSettingsToggle: () => void
  onSettingsClose: () => void
}

export function ViewPanel({
  scene,
  display,
  histogram,
  dtype,
  awaitingFrame,
  settingsOpen,
  onSettingsToggle,
  onSettingsClose,
}: ViewPanelProps) {
  usePlayback(awaitingFrame)

  return (
    <section className="viewpanel" aria-label="Image view">
      <ChannelBar
        histogram={histogram}
        dtype={dtype}
        settingsOpen={settingsOpen}
        onSettingsToggle={onSettingsToggle}
        onSettingsClose={onSettingsClose}
      />
      <Stage scene={scene} display={display} />
      <Transport />
    </section>
  )
}
