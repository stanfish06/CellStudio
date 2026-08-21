import { describe, expect, it } from 'vitest'
import { LARGE_STEP, SHORTCUTS, TOOL_KEYS, isToolEnabled, resolveKey } from './keymap'

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
})
