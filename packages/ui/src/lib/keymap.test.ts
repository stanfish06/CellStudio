import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  ASSIGN_LABELS_KEY,
  LARGE_STEP,
  RESET_VIEW_KEY,
  SHORTCUTS,
  TOOL_KEYS,
  UNLINK_KEY,
  isPaintTool,
  isToolEnabled,
  isTypingTarget,
  resolveKey,
} from './keymap'

describe('resolveKey', () => {
  it('maps 1-4 to the four views in order', () => {
    expect(resolveKey({ key: '1' })).toEqual({ kind: 'view', view: 'xy' })
    expect(resolveKey({ key: '2' })).toEqual({ kind: 'view', view: 'xz' })
    expect(resolveKey({ key: '3' })).toEqual({ kind: 'view', view: 'yz' })
    expect(resolveKey({ key: '4' })).toEqual({ kind: 'view', view: '3d' })
    expect(resolveKey({ key: '5' })).toBeNull()
  })

  it('steps t with the arrows and the slice axis with brackets', () => {
    expect(resolveKey({ key: 'ArrowRight' })).toEqual({ kind: 'stepT', delta: 1 })
    expect(resolveKey({ key: 'ArrowLeft' })).toEqual({ kind: 'stepT', delta: -1 })
    expect(resolveKey({ key: ']' })).toEqual({ kind: 'stepSlice', delta: 1 })
    expect(resolveKey({ key: '[' })).toEqual({ kind: 'stepSlice', delta: -1 })
  })

  it('multiplies every step by 10 with Shift', () => {
    expect(LARGE_STEP).toBe(10)
    expect(resolveKey({ key: 'ArrowRight', shiftKey: true })).toEqual({ kind: 'stepT', delta: 10 })
    expect(resolveKey({ key: 'ArrowLeft', shiftKey: true })).toEqual({ kind: 'stepT', delta: -10 })
    expect(resolveKey({ key: ']', shiftKey: true })).toEqual({ kind: 'stepSlice', delta: 10 })
    expect(resolveKey({ key: '[', shiftKey: true })).toEqual({ kind: 'stepSlice', delta: -10 })
  })

  it('maps every tool letter, case-insensitively', () => {
    for (const [tool, key] of Object.entries(TOOL_KEYS)) {
      expect(resolveKey({ key })).toEqual({ kind: 'tool', tool })
      expect(resolveKey({ key: key.toLowerCase() })).toEqual({ kind: 'tool', tool })
    }
  })

  it('enables the paint and link tools and leaves the unshipped ones disabled', () => {
    expect(resolveKey({ key: 'B' })).toEqual({ kind: 'tool', tool: 'brush' })
    expect(isToolEnabled('brush')).toBe(true)
    expect(isToolEnabled('eraser')).toBe(true)
    expect(isToolEnabled('link')).toBe(true)
    expect(isToolEnabled('pointer')).toBe(true)
    expect(isPaintTool('brush')).toBe(true)
    expect(isPaintTool('eraser')).toBe(true)
    expect(isPaintTool('pointer')).toBe(false)
  })

  it('resolves the unlink action on either case of X — an action, never a tool', () => {
    expect(UNLINK_KEY).toBe('X')
    expect(resolveKey({ key: 'X' })).toEqual({ kind: 'unlink' })
    expect(resolveKey({ key: 'x' })).toEqual({ kind: 'unlink' })
    expect(resolveKey({ key: 'x', metaKey: true })).toBeNull()
    expect(Object.values(TOOL_KEYS)).not.toContain(UNLINK_KEY)
  })

  it('adjusts the brush radius on both spellings of - and =', () => {
    expect(resolveKey({ key: '-' })).toEqual({ kind: 'brushRadius', delta: -1 })
    expect(resolveKey({ key: '=' })).toEqual({ kind: 'brushRadius', delta: 1 })
    expect(resolveKey({ key: '_', shiftKey: true })).toEqual({
      kind: 'brushRadius',
      delta: -LARGE_STEP,
    })
    expect(resolveKey({ key: '+', shiftKey: true })).toEqual({
      kind: 'brushRadius',
      delta: LARGE_STEP,
    })
  })

  it('deletes the selected mask on Delete and Backspace', () => {
    expect(resolveKey({ key: 'Delete' })).toEqual({ kind: 'deleteMask' })
    expect(resolveKey({ key: 'Backspace' })).toEqual({ kind: 'deleteMask' })
  })

  it('resolves undo and redo on either accelerator, and only unaltered', () => {
    expect(resolveKey({ key: 'z', ctrlKey: true })).toEqual({ kind: 'undo' })
    expect(resolveKey({ key: 'Z', metaKey: true })).toEqual({ kind: 'undo' })
    expect(resolveKey({ key: 'z', metaKey: true, shiftKey: true })).toEqual({ kind: 'redo' })
    expect(resolveKey({ key: 'Z', ctrlKey: true, shiftKey: true })).toEqual({ kind: 'redo' })
    expect(resolveKey({ key: 'z', ctrlKey: true, altKey: true })).toBeNull()
    expect(resolveKey({ key: 'z' })).toBeNull()
  })

  it('resets the view on either case of the reset key', () => {
    expect(RESET_VIEW_KEY).toBe('R')
    expect(resolveKey({ key: 'r' })).toEqual({ kind: 'resetView' })
    expect(resolveKey({ key: 'R' })).toEqual({ kind: 'resetView' })
    expect(resolveKey({ key: 'R', shiftKey: true })).toEqual({ kind: 'resetView' })
  })

  it('leaves a modified reset key to the menus', () => {
    expect(resolveKey({ key: 'r', metaKey: true })).toBeNull()
    expect(resolveKey({ key: 'R', ctrlKey: true })).toBeNull()
    expect(resolveKey({ key: 'r', altKey: true })).toBeNull()
    expect(resolveKey({ key: 'r', ctrlKey: true, shiftKey: true })).toBeNull()
  })

  it('opens the shortcuts dialog on ? and dismisses on Escape', () => {
    expect(resolveKey({ key: '?' })).toEqual({ kind: 'shortcuts' })
    expect(resolveKey({ key: 'Escape' })).toEqual({ kind: 'dismiss' })
  })

  it('lets Escape through modifiers but ignores accelerators', () => {
    expect(resolveKey({ key: 'Escape', metaKey: true })).toEqual({ kind: 'dismiss' })
    expect(resolveKey({ key: 'V', metaKey: true })).toBeNull()
    expect(resolveKey({ key: 'ArrowRight', ctrlKey: true })).toBeNull()
    expect(resolveKey({ key: '1', altKey: true })).toBeNull()
  })

  it('ignores unbound keys', () => {
    expect(resolveKey({ key: 'q' })).toBeNull()
    expect(resolveKey({ key: 'F5' })).toBeNull()
    expect(resolveKey({ key: 'Home' })).toBeNull()
  })
})

class FakeElement {
  constructor(
    readonly tagName: string,
    readonly isContentEditable = false,
  ) {}
}

// No DOM in this environment; `instanceof HTMLElement` needs a stand-in.
const target = (tagName: string, isContentEditable = false) =>
  new FakeElement(tagName, isContentEditable) as unknown as EventTarget

describe('isTypingTarget', () => {
  beforeEach(() => vi.stubGlobal('HTMLElement', FakeElement))
  afterEach(() => vi.unstubAllGlobals())

  it('claims the keys typed into a field, including the reset key', () => {
    expect(resolveKey({ key: 'r' })).toEqual({ kind: 'resetView' })
    expect(isTypingTarget(target('INPUT'))).toBe(true)
    expect(isTypingTarget(target('TEXTAREA'))).toBe(true)
    expect(isTypingTarget(target('SELECT'))).toBe(true)
    expect(isTypingTarget(target('DIV', true))).toBe(true)
  })

  it('leaves the canvas and non-elements to the app', () => {
    expect(isTypingTarget(target('CANVAS'))).toBe(false)
    expect(isTypingTarget(target('BUTTON'))).toBe(false)
    expect(isTypingTarget(null)).toBe(false)
  })
})

describe('SHORTCUTS', () => {
  it('lists a binding for every tool letter plus the view, frame and slice keys', () => {
    const text = SHORTCUTS.map((s) => `${s.action} ${s.keys}`).join(' | ')
    for (const key of Object.values(TOOL_KEYS)) expect(text).toContain(key)
    expect(text).toContain('1–4')
    expect(text).toContain('← →')
    expect(text).toContain('[ ]')
    expect(text).toContain('Shift')
    expect(text).toContain('Esc')
  })

  it('lists the reset binding with the navigation rows', () => {
    const index = SHORTCUTS.findIndex((s) => s.action === 'Reset 3D view')
    expect(SHORTCUTS[index]?.keys).toBe(RESET_VIEW_KEY)
    expect(index).toBeLessThan(SHORTCUTS.findIndex((s) => s.action === 'Pointer / pan'))
  })

  it('resolves every single key it advertises', () => {
    for (const row of SHORTCUTS) {
      for (const key of row.keys.split(' / ')) {
        if (key.length !== 1) continue
        expect(resolveKey({ key }), `${row.action} (${key})`).not.toBeNull()
      }
    }
  })

  it('advertises the mask edit bindings, including the pointer gestures', () => {
    const row = (action: string) => SHORTCUTS.find((s) => s.action === action)
    expect(row('Brush radius down / up')?.keys).toBe('- / =')
    expect(row('Brush radius (over the view)')?.keys).toBe('Shift+wheel')
    expect(row('Pan or orbit while painting')?.keys).toBe('Alt+drag')
    expect(row('Delete selected mask')?.keys).toBe('Del / Backspace')
    expect(row('Undo')?.keys).toBe('Ctrl/Cmd+Z')
    expect(row('Redo')?.keys).toBe('Ctrl/Cmd+Shift+Z')
  })

  it('resolves the named keys it advertises', () => {
    expect(resolveKey({ key: 'Delete' })).toEqual({ kind: 'deleteMask' })
    expect(resolveKey({ key: 'Backspace' })).toEqual({ kind: 'deleteMask' })
    expect(resolveKey({ key: 'z', ctrlKey: true })).toEqual({ kind: 'undo' })
    expect(resolveKey({ key: 'z', metaKey: true, shiftKey: true })).toEqual({ kind: 'redo' })
  })
})

describe('assign labels', () => {
  it('binds A in either case, rejects it under a modifier, and lists it', () => {
    expect(ASSIGN_LABELS_KEY).toBe('A')
    expect(resolveKey({ key: 'a' })).toEqual({ kind: 'assignLabels' })
    expect(resolveKey({ key: 'A' })).toEqual({ kind: 'assignLabels' })
    expect(resolveKey({ key: 'a', metaKey: true })).toBeNull()
    expect(resolveKey({ key: 'a', ctrlKey: true })).toBeNull()
    expect(SHORTCUTS.some((row) => row.keys === ASSIGN_LABELS_KEY)).toBe(true)
    expect(resolveKey({ key: 'u' })).toEqual({ kind: 'unassignLabels' })
    expect(resolveKey({ key: 'g' })).toBeNull()
    expect(resolveKey({ key: 'i' })).toBeNull()
  })
})
