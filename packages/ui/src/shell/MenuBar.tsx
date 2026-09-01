import { useEffect, useRef, useState } from 'react'
import type { MenuId } from '../types'

export interface MenuBarProps {
  onMenu?: (menu: MenuId) => void
  /** File → "Open dataset…"; falls back to `onMenu('file')`. */
  onOpenDataset?: () => void
  /** File → "Import tracking…". */
  onImportTracking?: () => void
  /** A project is open and its graph is empty — the import precondition. */
  canImportTracking?: boolean
  /** Why the import item is disabled, shown as its tooltip. */
  importTrackingHint?: string
  /** Edit → "Save tracking snapshot". */
  onSaveTrackingSnapshot?: () => void
  /** The project has a track graph (ProjectInfo.hasGraph); gates the snapshot item. */
  canSaveTrackingSnapshot?: boolean
}

interface MenuItem {
  label: string
  enabled: boolean
  hint?: string
  onSelect: () => void
}

const DROPDOWN_STYLE = {
  position: 'absolute',
  top: '100%',
  left: 0,
  zIndex: 30,
  minWidth: 200,
  padding: 2,
  background: 'var(--bg-1)',
  border: '1px solid var(--line)',
  borderRadius: 3,
  display: 'grid',
} as const

export function MenuBar({
  onMenu,
  onOpenDataset,
  onImportTracking,
  canImportTracking = false,
  importTrackingHint,
  onSaveTrackingSnapshot,
  canSaveTrackingSnapshot = false,
}: MenuBarProps) {
  const [open, setOpen] = useState<MenuId | null>(null)
  const nav = useRef<HTMLElement>(null)

  // a menu left open over the viewer swallows the next click; close on any press outside
  useEffect(() => {
    if (open === null) return
    const close = (event: PointerEvent) => {
      if (!nav.current?.contains(event.target as Node)) setOpen(null)
    }
    document.addEventListener('pointerdown', close)
    return () => document.removeEventListener('pointerdown', close)
  }, [open])

  const items: Record<MenuId, readonly MenuItem[]> = {
    file: [
      {
        label: 'Open dataset…',
        enabled: true,
        onSelect: () => (onOpenDataset ?? (() => onMenu?.('file')))(),
      },
      {
        label: 'Import tracking…',
        enabled: canImportTracking && onImportTracking !== undefined,
        hint: importTrackingHint,
        onSelect: () => onImportTracking?.(),
      },
    ],
    edit: onSaveTrackingSnapshot
      ? [
          {
            label: 'Save tracking snapshot',
            enabled: canSaveTrackingSnapshot,
            hint: 'The project has no track graph yet',
            onSelect: onSaveTrackingSnapshot,
          },
        ]
      : [],
    view: [],
    help: [],
  }

  const MENUS: readonly { id: MenuId; label: string; plain: boolean }[] = [
    { id: 'file', label: 'File', plain: false },
    { id: 'edit', label: 'Edit', plain: false },
    { id: 'view', label: 'View', plain: true },
    { id: 'help', label: 'Help', plain: true },
  ]

  return (
    <nav
      className="menubar"
      aria-label="Application menu"
      ref={nav}
      onKeyDown={(event) => {
        if (event.key === 'Escape') setOpen(null)
      }}
    >
      {MENUS.map((menu) => {
        const rows = items[menu.id]
        const hasDropdown = rows.length > 0
        return (
          <div key={menu.id} style={{ position: 'relative' }}>
            <button
              type="button"
              className="menu-button"
              disabled={!hasDropdown && !menu.plain}
              aria-haspopup={hasDropdown ? 'menu' : undefined}
              aria-expanded={hasDropdown ? open === menu.id : undefined}
              onClick={() => {
                if (hasDropdown) setOpen(open === menu.id ? null : menu.id)
                else onMenu?.(menu.id)
              }}
            >
              {menu.label}
            </button>
            {hasDropdown && open === menu.id && (
              <div role="menu" style={DROPDOWN_STYLE}>
                {rows.map((row) => (
                  <button
                    key={row.label}
                    type="button"
                    role="menuitem"
                    className="menu-button"
                    style={{ textAlign: 'left' }}
                    disabled={!row.enabled}
                    title={row.enabled ? undefined : row.hint}
                    onClick={() => {
                      setOpen(null)
                      row.onSelect()
                    }}
                  >
                    {row.label}
                  </button>
                ))}
              </div>
            )}
          </div>
        )
      })}
      <span className="menu-spacer" />
    </nav>
  )
}
