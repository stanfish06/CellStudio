import type { Tool } from '@cellstudio/viewer'
import { HelpIcon, TOOL_ICONS } from '../icons'
import { TOOL_KEYS, TOOL_LABELS, isToolEnabled } from '../lib/keymap'

interface ToolGroup {
  caption: string
  tools: readonly Tool[]
}

const GROUPS: readonly ToolGroup[] = [
  { caption: 'Navigation', tools: ['pointer', 'pan'] },
  { caption: 'Mask edit', tools: ['brush', 'eraser', 'fill', 'pick'] },
  { caption: 'Track edit', tools: ['link', 'cut'] },
]

export interface RibbonProps {
  tool: Tool
  onTool: (tool: Tool) => void
  onShortcuts: () => void
}

export function Ribbon({ tool, onTool, onShortcuts }: RibbonProps) {
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
                disabled={!isToolEnabled(t)}
                onClick={() => onTool(t)}
              />
            ))}
            {group.caption === 'Navigation' ? (
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

interface ToolButtonProps {
  tool: Tool
  active: boolean
  disabled: boolean
  onClick: () => void
}

function ToolButton({ tool, active, disabled, onClick }: ToolButtonProps) {
  const Icon = TOOL_ICONS[tool]
  const label = TOOL_LABELS[tool]
  const key = TOOL_KEYS[tool]
  return (
    <button
      type="button"
      className={active ? 'ribbon-tool active' : 'ribbon-tool'}
      title={disabled ? `${label} (${key}) — ships with a later phase` : `${label} (${key})`}
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
