import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  LARGE_STEP,
  RESET_VIEW_KEY,
  SHORTCUTS,
  TOOL_KEYS,
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

  it('resolves editing tools but reports them as not enabled', () => {
    expect(resolveKey({ key: 'B' })).toEqual({ kind: 'tool', tool: 'brush' })
    expect(isToolEnabled('brush')).toBe(false)
    expect(isToolEnabled('cut')).toBe(false)
    expect(isToolEnabled('pointer')).toBe(true)
    expect(isToolEnabled('pan')).toBe(true)
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
})
