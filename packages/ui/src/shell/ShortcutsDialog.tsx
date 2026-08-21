import { SHORTCUTS } from '../lib/keymap'

export interface ShortcutsDialogProps {
  onClose: () => void
}

export function ShortcutsDialog({ onClose }: ShortcutsDialogProps) {
  return (
    <div
      className="shortcut-modal"
      role="dialog"
      aria-modal="true"
      aria-label="Keyboard shortcuts"
      onClick={onClose}
    >
      <div className="shortcut-card" onClick={(e) => e.stopPropagation()}>
        <h2>Keyboard shortcuts</h2>
        <div className="shortcut-grid">
          {SHORTCUTS.map((row) => (
            <div className="shortcut" key={row.action}>
              <span>{row.action}</span>
              <kbd>{row.keys}</kbd>
            </div>
          ))}
        </div>
        <div className="panel-actions shortcut-actions">
          <button type="button" className="small-button primary" autoFocus onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  )
}
