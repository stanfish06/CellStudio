import type { ActiveView, Tool } from '@cellstudio/viewer'

export const VIEW_KEYS: readonly ActiveView[] = ['xy', 'xz', 'yz', '3d']

export const VIEW_LABELS: Record<ActiveView, string> = {
  xy: 'XY',
  xz: 'XZ',
  yz: 'YZ',
  '3d': '3D',
}

export const TOOL_KEYS: Record<Tool, string> = {
  pointer: 'V',
  pan: 'H',
  brush: 'B',
  eraser: 'E',
  fill: 'G',
  pick: 'I',
  link: 'L',
  cut: 'X',
}

export const TOOL_LABELS: Record<Tool, string> = {
  pointer: 'Pointer',
  pan: 'Pan',
  brush: 'Brush',
  eraser: 'Eraser',
  fill: 'Fill',
  pick: 'Pick label',
  link: 'Link',
  cut: 'Cut link',
}

export const ENABLED_TOOLS: readonly Tool[] = ['pointer', 'pan', 'brush', 'eraser']

/** Tools that stamp the brush radius; the radius control follows this set. */
export const PAINT_TOOLS: readonly Tool[] = ['brush', 'eraser']

export const LARGE_STEP = 10

/** Returns the 3D camera to the fit pose; case-insensitively. */
export const RESET_VIEW_KEY = 'R'

export type KeyAction =
  | { kind: 'view'; view: ActiveView }
  | { kind: 'stepT'; delta: number }
  | { kind: 'stepSlice'; delta: number }
  | { kind: 'tool'; tool: Tool }
  | { kind: 'brushRadius'; delta: number }
  | { kind: 'deleteMask' }
  | { kind: 'undo' }
  | { kind: 'redo' }
  | { kind: 'resetView' }
  | { kind: 'shortcuts' }
  | { kind: 'dismiss' }

export interface KeyLike {
  key: string
  shiftKey?: boolean
  ctrlKey?: boolean
  metaKey?: boolean
  altKey?: boolean
}

const TOOL_BY_KEY = new Map<string, Tool>(
  (Object.entries(TOOL_KEYS) as [Tool, string][]).map(([tool, key]) => [key, tool]),
)

export function isToolEnabled(tool: Tool): boolean {
  return ENABLED_TOOLS.includes(tool)
}

export function isPaintTool(tool: Tool): boolean {
  return PAINT_TOOLS.includes(tool)
}

export function resolveKey(e: KeyLike): KeyAction | null {
  if (e.key === 'Escape') return { kind: 'dismiss' }

  // Undo and redo are the only chords; every other binding is rejected when modified,
  // so accelerators keep reaching the menus.
  const accel = e.ctrlKey === true || e.metaKey === true
  if (accel && e.altKey !== true && e.key.length === 1 && e.key.toUpperCase() === 'Z') {
    return e.shiftKey === true ? { kind: 'redo' } : { kind: 'undo' }
  }
  if (accel || e.altKey === true) return null
  if (e.key === '?') return { kind: 'shortcuts' }

  const viewIndex = '1234'.indexOf(e.key)
  if (e.key.length === 1 && viewIndex >= 0) {
    const view = VIEW_KEYS[viewIndex]
    return view ? { kind: 'view', view } : null
  }

  const delta = e.shiftKey === true ? LARGE_STEP : 1
  switch (e.key) {
    case 'ArrowRight':
      return { kind: 'stepT', delta }
    case 'ArrowLeft':
      return { kind: 'stepT', delta: -delta }
    case ']':
      return { kind: 'stepSlice', delta }
    case '[':
      return { kind: 'stepSlice', delta: -delta }
    // Shifted on a US layout these arrive as `_` and `+`, so both spellings bind.
    case '=':
    case '+':
      return { kind: 'brushRadius', delta }
    case '-':
    case '_':
      return { kind: 'brushRadius', delta: -delta }
    case 'Delete':
    case 'Backspace':
      return { kind: 'deleteMask' }
  }

  if (e.key.length !== 1) return null
  const letter = e.key.toUpperCase()
  if (letter === RESET_VIEW_KEY) return { kind: 'resetView' }
  const tool = TOOL_BY_KEY.get(letter)
  return tool ? { kind: 'tool', tool } : null
}

/** Keys typed into a field belong to the field; only Escape still reaches the app. */
export function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false
  const tag = target.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || target.isContentEditable
}

export interface ShortcutRow {
  action: string
  keys: string
}

export const SHORTCUTS: readonly ShortcutRow[] = [
  { action: 'XY / XZ / YZ / 3D', keys: '1–4' },
  { action: 'Previous / next frame', keys: '← →' },
  { action: 'Previous / next slice', keys: '[ ]' },
  { action: `Large step (${LARGE_STEP}×)`, keys: 'Shift' },
  { action: 'Reset 3D view', keys: RESET_VIEW_KEY },
  { action: 'Pointer / pan', keys: `${TOOL_KEYS.pointer} / ${TOOL_KEYS.pan}` },
  { action: 'Brush / eraser', keys: `${TOOL_KEYS.brush} / ${TOOL_KEYS.eraser}` },
  { action: 'Fill / pick label', keys: `${TOOL_KEYS.fill} / ${TOOL_KEYS.pick}` },
  { action: 'Link / cut link', keys: `${TOOL_KEYS.link} / ${TOOL_KEYS.cut}` },
  { action: 'Brush radius down / up', keys: '- / =' },
  { action: 'Brush radius (over the view)', keys: 'Shift+wheel' },
  { action: 'Pan or orbit while painting', keys: 'Alt+drag' },
  { action: 'Delete selected mask', keys: 'Del / Backspace' },
  { action: 'Undo', keys: 'Ctrl/Cmd+Z' },
  { action: 'Redo', keys: 'Ctrl/Cmd+Shift+Z' },
  { action: 'Keyboard shortcuts', keys: '?' },
  { action: 'Close dialog or popover', keys: 'Esc' },
]
