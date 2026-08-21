import type { MenuId } from '../types'

const MENUS: readonly { id: MenuId; label: string; enabled: boolean }[] = [
  { id: 'file', label: 'File', enabled: true },
  { id: 'edit', label: 'Edit', enabled: false },
  { id: 'view', label: 'View', enabled: true },
  { id: 'help', label: 'Help', enabled: true },
]

export interface MenuBarProps {
  onMenu?: (menu: MenuId) => void
}

export function MenuBar({ onMenu }: MenuBarProps) {
  return (
    <nav className="menubar" aria-label="Application menu">
      {MENUS.map((menu) => (
        <button
          key={menu.id}
          type="button"
          className="menu-button"
          disabled={!menu.enabled}
          onClick={() => onMenu?.(menu.id)}
        >
          {menu.label}
        </button>
      ))}
      <span className="menu-spacer" />
    </nav>
  )
}
