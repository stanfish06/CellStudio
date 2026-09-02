import type { CursorSample } from '../types'

export interface StatusBarProps {
  cursor: CursorSample | null
  /** Most recent failure to reach the user; a later success clears it. */
  error?: string | null
  /** Non-failure completion message — e.g. the path a tracking snapshot was written to. */
  notice?: string | null
}

/** Pointer position on the left; the latest failure or notice on the right. Backend, job,
 * and pending-write state live in the Inspect tab's Project status block. */
export function StatusBar({ cursor, error = null, notice = null }: StatusBarProps) {
  return (
    <footer className="statusbar">
      <div className="status-left">
        {cursor ? (
          <>
            <span>
              X <strong>{cursor.x}</strong>
            </span>
            <span>
              Y <strong>{cursor.y}</strong>
            </span>
            <span>
              Z <strong>{cursor.z}</strong>
            </span>
          </>
        ) : null}
      </div>
      {error ? (
        <span className="write-status failed" title={error}>
          <i className="dot down" />
          {error}
        </span>
      ) : notice ? (
        <span className="job-status" title={notice}>
          <i className="dot" />
          {notice}
        </span>
      ) : null}
    </footer>
  )
}
