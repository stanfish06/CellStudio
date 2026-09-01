import { BRUSH_RADIUS_MAX, BRUSH_RADIUS_MIN, type Tool } from '@cellstudio/viewer'
import { useState } from 'react'
import { DeleteMaskIcon, HelpIcon, ResetViewIcon, TOOL_ICONS, UnlinkIcon } from '../icons'
import {
  RESET_VIEW_KEY,
  TOOL_KEYS,
  TOOL_LABELS,
  UNLINK_KEY,
  isPaintTool,
  isToolEnabled,
} from '../lib/keymap'

interface ToolGroup {
  caption: string
  tools: readonly Tool[]
}

const GROUPS: readonly ToolGroup[] = [
  { caption: 'Navigation', tools: ['pointer', 'pan'] },
  { caption: 'Mask edit', tools: ['brush', 'eraser', 'fill', 'pick'] },
  { caption: 'Track edit', tools: ['link'] },
]

export interface RibbonProps {
  tool: Tool
  onTool: (tool: Tool) => void
  onShortcuts: () => void
  onResetView: () => void
  resetEnabled: boolean
  brushRadius: number
  onBrushRadius: (radius: number) => void
  onDeleteMask: () => void
  /** True only while a cell that exists on the current frame is selected. */
  deleteEnabled: boolean
  /** Link's arming precondition: the project has a graph and a cell is selected. */
  linkEnabled: boolean
  onUnlink: () => void
  /** What Unlink would act on: the selected edge, or the selected cell's whole track. */
  unlinkTarget?: 'edge' | 'track'
  /** Unlink acts on the selection; enabled exactly while a cell or an edge is selected. */
  unlinkEnabled: boolean
}

export function Ribbon({
  tool,
  onTool,
  onShortcuts,
  onResetView,
  resetEnabled,
  brushRadius,
  onBrushRadius,
  onDeleteMask,
  deleteEnabled,
  linkEnabled,
  onUnlink,
  unlinkTarget = 'track',
  unlinkEnabled,
}: RibbonProps) {
  return (
    <div className="ribbon" role="toolbar" aria-label="Editing tools">
      {GROUPS.map((group) => {
        const captionId = `ribbon-${group.caption.replace(/\s+/g, '-').toLowerCase()}`
        return (
          <div
            className="ribbon-group"
            key={group.caption}
            role="group"
            aria-labelledby={captionId}
          >
            {group.tools.map((t) => (
              <ToolButton
                key={t}
                tool={t}
                active={t === tool}
                disabled={t === 'link' ? !linkEnabled : !isToolEnabled(t)}
                disabledHint={
                  t === 'link'
                    ? 'select a cell in a project with tracks'
                    : 'ships with a later phase'
                }
                onClick={() => onTool(t)}
              />
            ))}
            {group.caption === 'Navigation' ? (
              <>
                <button
                  type="button"
                  className="ribbon-tool"
                  title={
                    resetEnabled
                      ? `Reset view (${RESET_VIEW_KEY})`
                      : `Reset view (${RESET_VIEW_KEY}) — 3D view only`
                  }
                  disabled={!resetEnabled}
                  onClick={onResetView}
                >
                  <ResetViewIcon />
                  <span className="ribbon-label">Reset view</span>
                  <span className="key">{RESET_VIEW_KEY}</span>
                </button>
                <button
                  type="button"
                  className="ribbon-tool"
                  title="Keyboard shortcuts (?)"
                  onClick={onShortcuts}
                >
                  <HelpIcon />
                  <span className="ribbon-label">Shortcuts</span>
                  <span className="key">?</span>
                </button>
              </>
            ) : null}
            {group.caption === 'Mask edit' ? (
              <>
                <button
                  type="button"
                  className="ribbon-tool"
                  title={
                    deleteEnabled
                      ? 'Delete mask on this frame (Del)'
                      : 'Delete mask on this frame (Del) — select a cell on this frame'
                  }
                  disabled={!deleteEnabled}
                  onClick={onDeleteMask}
                >
                  <DeleteMaskIcon />
                  <span className="ribbon-label">Delete mask</span>
                  <span className="key">Del</span>
                </button>
                <BrushRadius
                  radius={brushRadius}
                  onRadius={onBrushRadius}
                  disabled={!isPaintTool(tool)}
                />
              </>
            ) : null}
            {group.caption === 'Track edit' ? (
              <button
                type="button"
                className="ribbon-tool"
                title={
                  unlinkEnabled
                    ? unlinkTarget === 'edge'
                      ? `Cut the selected link (${UNLINK_KEY})`
                      : `Unlink selected track (${UNLINK_KEY})`
                    : `Unlink (${UNLINK_KEY}) — select a cell or a trail edge`
                }
                disabled={!unlinkEnabled}
                onClick={onUnlink}
              >
                <UnlinkIcon />
                <span className="ribbon-label">Unlink</span>
                <span className="key">{UNLINK_KEY}</span>
              </button>
            ) : null}
            <span className="ribbon-group-title" id={captionId}>
              {group.caption}
            </span>
          </div>
        )
      })}
    </div>
  )
}

interface BrushRadiusProps {
  radius: number
  onRadius: (radius: number) => void
  disabled: boolean
}

/** The slot is present for every tool and disabled outside the paint tools, so the shell
 * keeps its geometry as the tool changes. */
function BrushRadius({ radius, onRadius, disabled }: BrushRadiusProps) {
  const [draft, setDraft] = useState<string | null>(null)

  const commit = () => {
    if (draft === null) return
    const parsed = Number(draft)
    setDraft(null)
    // The store clamps to 1–200; a half-typed or empty field commits nothing.
    if (draft.trim() !== '' && Number.isFinite(parsed)) onRadius(parsed)
  }

  return (
    <div className={disabled ? 'ribbon-field disabled' : 'ribbon-field'}>
      <label htmlFor="brush-radius">Radius</label>
      <input
        id="brush-radius"
        type="number"
        min={BRUSH_RADIUS_MIN}
        max={BRUSH_RADIUS_MAX}
        step={1}
        value={draft ?? radius}
        disabled={disabled}
        title={`Brush radius in dataset pixels (- / =), ${BRUSH_RADIUS_MIN}–${BRUSH_RADIUS_MAX}`}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit()
          if (e.key === 'Escape') setDraft(null)
        }}
      />
      <input
        type="range"
        aria-label="Brush radius"
        min={BRUSH_RADIUS_MIN}
        max={BRUSH_RADIUS_MAX}
        step={1}
        value={radius}
        disabled={disabled}
        onChange={(e) => onRadius(Number(e.target.value))}
      />
    </div>
  )
}

interface ToolButtonProps {
  tool: Tool
  active: boolean
  disabled: boolean
  /** Why the button is disabled, appended to the tooltip. */
  disabledHint: string
  onClick: () => void
}

function ToolButton({ tool, active, disabled, disabledHint, onClick }: ToolButtonProps) {
  const Icon = TOOL_ICONS[tool]
  const label = TOOL_LABELS[tool]
  const key = TOOL_KEYS[tool]
  return (
    <button
      type="button"
      className={active ? 'ribbon-tool active' : 'ribbon-tool'}
      title={disabled ? `${label} (${key}) — ${disabledHint}` : `${label} (${key})`}
      aria-pressed={active}
      disabled={disabled}
      onClick={onClick}
    >
      <Icon />
      <span className="ribbon-label">{label}</span>
      <span className="key">{key}</span>
    </button>
  )
}
