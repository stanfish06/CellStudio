import { sliceAxis, useNav } from '@cellstudio/viewer'
import type { ReactNode } from 'react'
import { hudChips, pixelsPerUm, scaleBar, type Chip } from '../lib/hud'
import { cssVars, type DisplayState } from '../types'

export interface StageProps {
  scene?: ReactNode
  display: DisplayState
}

export function Stage({ scene, display }: StageProps) {
  const project = useNav((s) => s.project)
  const activeView = useNav((s) => s.activeView)
  const slices = useNav((s) => s.slices)
  const t = useNav((s) => s.t)
  const volumeCamera = useNav((s) => s.volume.camera)

  const dims = project?.dims ?? null
  const orientation = activeView === '3d' ? null : activeView
  const axis = orientation ? sliceAxis(orientation) : null
  const zoom = activeView === '3d' && volumeCamera ? volumeCamera.zoom : display.zoom

  const chips = hudChips({
    activeView,
    slice:
      orientation && axis
        ? {
            axis,
            index: slices[orientation].index,
            max: Math.max(0, (dims?.[axis] ?? 1) - 1),
          }
        : null,
    t,
    tMax: Math.max(0, (dims?.t ?? 1) - 1),
    level: display.level,
    zoom,
    scale: project?.scale ?? null,
  })
  const bar = scaleBar(pixelsPerUm(project?.scale ?? null, zoom))

  return (
    <div className="stage">
      <div className="canvas-wrap">
        <div className="canvas-scene">{scene}</div>
        <div className="canvas-top-left">
          <HudChip chip={chips.orientation} />
          <span className="hud">{chips.level}</span>
        </div>
        <div className="canvas-top-right">
          <HudChip chip={chips.frame} />
          <span className="hud">{chips.voxel}</span>
        </div>
        {bar ? (
          <div className="scale-bar" style={cssVars({ '--bar-width': `${bar.lengthPx}px` })}>
            {bar.label}
          </div>
        ) : null}
      </div>
    </div>
  )
}

function HudChip({ chip }: { chip: Chip }) {
  return (
    <span className="hud">
      {chip.prefix}
      <strong>{chip.emphasis}</strong>
      {chip.suffix}
    </span>
  )
}
