import type { JobState } from '@cellstudio/api-client'
import { activeJob } from '../lib/advisories'
import { channelTag } from '../lib/channels'
import { formatInt, formatPercent } from '../lib/format'
import type { BackendState, CursorSample, PerfSample } from '../types'

const BACKEND_TEXT: Record<BackendState, string> = {
  starting: 'Backend starting',
  ready: 'Backend ready',
  down: 'Backend down',
  fatal: 'Backend failed',
}

const JOB_TEXT: Record<JobState['kind'], string> = {
  rechunk: 'Re-chunking',
  proxy: 'Building proxy',
  inventory: 'Inventorying labels',
  'import-tracks': 'Importing tracks',
  'import-labels': 'Importing masks',
  export: 'Exporting',
}

export interface StatusBarProps {
  cursor: CursorSample | null
  activeChannel: number
  backend: BackendState
  jobs: readonly JobState[]
  perf: PerfSample | null
  /** Edits posted but not yet acknowledged (`ProjectStatus.pendingWrites`). */
  pendingWrites: number
  /** Most recent failure to reach the user; a later success clears it. */
  error?: string | null
  /** Non-failure completion message — e.g. the path a tracking snapshot was written to. */
  notice?: string | null
}

export function StatusBar({
  cursor,
  activeChannel,
  backend,
  jobs,
  perf,
  pendingWrites,
  error = null,
  notice = null,
}: StatusBarProps) {
  const job = activeJob(jobs)
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
            <span>
              {channelTag(activeChannel)}{' '}
              <strong>{cursor.value === null ? '—' : formatInt(cursor.value)}</strong>
            </span>
            <span>
              Label <strong>{cursor.labelId ?? '—'}</strong>
            </span>
            <span>
              Track <strong>{cursor.trackId === null ? '—' : `T-${pad(cursor.trackId)}`}</strong>
            </span>
          </>
        ) : (
          <span>Cursor outside the image</span>
        )}
      </div>
      <span className="backend-status">
        <i
          className={
            backend === 'ready' ? 'dot' : backend === 'starting' ? 'dot progress' : 'dot down'
          }
        />
        {BACKEND_TEXT[backend]}
      </span>
      <span className="job-status" title={notice ?? undefined}>
        {job ? (
          <>
            <i className="dot progress" />
            {JOB_TEXT[job.kind]} {formatPercent(job.progress)}
          </>
        ) : notice ? (
          <>
            <i className="dot" />
            {notice}
          </>
        ) : (
          <>
            <i className="dot" />
            No background jobs
          </>
        )}
      </span>
      <span className={error ? 'write-status failed' : 'write-status'} title={error ?? undefined}>
        {error ? (
          <>
            <i className="dot down" />
            {error}
          </>
        ) : pendingWrites > 0 ? (
          <>
            <i className="dot progress" />
            {pendingWrites} pending {pendingWrites === 1 ? 'write' : 'writes'}
          </>
        ) : (
          <>
            <i className="dot" />
            No pending writes
          </>
        )}
      </span>
      <span>{perf ? `${perf.fps.toFixed(1)} fps · ${perf.frameMs.toFixed(1)} ms` : '— fps'}</span>
    </footer>
  )
}

function pad(trackId: number): string {
  return String(trackId).padStart(4, '0')
}
