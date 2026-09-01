import type { CellRow, JobState } from '@cellstudio/api-client'
import { AXIS_SCALE_MAX, AXIS_SCALE_MIN, useNav, type AxisScale } from '@cellstudio/viewer'
import { useState } from 'react'
import { AlertIcon } from '../icons'
import { advisories, rechunkJob } from '../lib/advisories'
import { AXIS_SCALE_KEYS, isPhysicalScale, parseAxisScale } from '../lib/axisScale'
import { basename, formatInt, formatPercent, formatShape, formatVoxelSize } from '../lib/format'
import { isLowConfidence } from '../lib/lineage'
import type { BackendState, ProjectStatus } from '../types'

const BACKEND_TEXT: Record<BackendState, string> = {
  starting: 'Starting',
  ready: 'Connected',
  down: 'Disconnected',
  fatal: 'Failed',
}

export interface InspectPanelProps {
  selection: CellRow | null
  jobs: readonly JobState[]
  backend: BackendState
  status: ProjectStatus
}

export function InspectPanel({ selection, jobs, backend, status }: InspectPanelProps) {
  const project = useNav((s) => s.project)
  const overlays = useNav((s) => s.overlays)
  const setOverlays = useNav((s) => s.setOverlays)
  const jumpToCell = useNav((s) => s.jumpToCell)
  const cards = advisories(project, jobs)
  const working = rechunkJob(jobs) ?? jobs.find((j) => j.kind === 'proxy' && j.status === 'running')

  return (
    <section className="panel">
      <div className="section">
        <div className="section-title">
          Selected cell
          <span className="minor">{selection ? `CELL ${selection.id}` : 'NONE'}</span>
        </div>
        {selection ? (
          <>
            <div className="kv">
              <span className="k">Track</span>
              <span className="v">
                {selection.trackId === null
                  ? '—'
                  : `T-${String(selection.trackId).padStart(4, '0')}`}
              </span>
              <span className="k">Frame</span>
              <span className="v">{selection.t}</span>
              <span className="k">Centroid</span>
              <span className="v">
                {selection.centroid ? selection.centroid.map((v) => Math.round(v)).join(', ') : '—'}
              </span>
              <span className="k">Area</span>
              <span className="v">
                {selection.area === null ? '—' : `${formatInt(selection.area)} px`}
              </span>
              <span className="k">Confidence</span>
              <span className={isLowConfidence(selection.confidence) ? 'v warn' : 'v'}>
                {selection.confidence === null ? '—' : selection.confidence.toFixed(2)}
              </span>
              <span className="k">State</span>
              <span className="v">{selection.state ?? 'normal'}</span>
            </div>
            <div className="panel-actions">
              <button
                type="button"
                className="small-button primary"
                onClick={() => jumpToCell(selection)}
              >
                Jump to cell
              </button>
            </div>
          </>
        ) : (
          <p className="empty-note">Click a cell in the view to inspect it.</p>
        )}
      </div>

      <div className="section">
        <div className="section-title">Overlays</div>
        <SliderRow
          label="Segmentation"
          value={Math.round(overlays.labels.opacity * 100)}
          min={0}
          max={100}
          readout={`${Math.round(overlays.labels.opacity * 100)}%`}
          onChange={(v) => setOverlays({ labels: { ...overlays.labels, opacity: v / 100 } })}
        />
        <SliderRow
          label="Tracks"
          value={Math.round(overlays.tracks.opacity * 100)}
          min={0}
          max={100}
          readout={`${Math.round(overlays.tracks.opacity * 100)}%`}
          onChange={(v) => setOverlays({ tracks: { ...overlays.tracks, opacity: v / 100 } })}
        />
        <SliderRow
          label="Dot size"
          value={overlays.tracks.dotSize}
          min={1}
          max={20}
          readout={`${overlays.tracks.dotSize} px`}
          onChange={(v) => setOverlays({ tracks: { ...overlays.tracks, dotSize: v } })}
        />
        <SliderRow
          label="Trail"
          value={overlays.tracks.trail}
          min={1}
          max={20}
          readout={`−${overlays.tracks.trail} T`}
          onChange={(v) => setOverlays({ tracks: { ...overlays.tracks, trail: v } })}
        />
        <div className="opacity-row">
          <span>Decay</span>
          <input
            type="checkbox"
            aria-label="Trail decay"
            checked={overlays.tracks.fade.on}
            onChange={(e) =>
              setOverlays({
                tracks: {
                  ...overlays.tracks,
                  fade: { ...overlays.tracks.fade, on: e.target.checked },
                },
              })
            }
          />
          <span>{overlays.tracks.fade.on ? 'On' : 'Off'}</span>
        </div>
        <SliderRow
          label="Decay max"
          value={Math.round(overlays.tracks.fade.max * 100)}
          min={0}
          max={100}
          readout={`${Math.round(overlays.tracks.fade.max * 100)}%`}
          onChange={(v) =>
            setOverlays({
              tracks: {
                ...overlays.tracks,
                fade: {
                  ...overlays.tracks.fade,
                  max: Math.max(v / 100, overlays.tracks.fade.min),
                },
              },
            })
          }
        />
        <SliderRow
          label="Decay min"
          value={Math.round(overlays.tracks.fade.min * 100)}
          min={0}
          max={100}
          readout={`${Math.round(overlays.tracks.fade.min * 100)}%`}
          onChange={(v) =>
            setOverlays({
              tracks: {
                ...overlays.tracks,
                fade: {
                  ...overlays.tracks.fade,
                  min: Math.min(v / 100, overlays.tracks.fade.max),
                },
              },
            })
          }
        />
      </div>

      <DisplayScaleSection />

      <div className="section">
        <div className="section-title">
          Dataset
          <span className="minor">{project ? 'OME-ZARR' : ''}</span>
        </div>
        {project ? (
          <div className="kv">
            <span className="k">Shape</span>
            <span className="v">{formatShape(project.dims)}</span>
            <span className="k">Source</span>
            <span className="v">{basename(project.sourcePath)}</span>
            <span className="k">Pyramid</span>
            <span className="v">
              {project.levels.length} {project.levels.length === 1 ? 'level' : 'levels'}
            </span>
            <span className="k">Voxel size</span>
            <span className={project.scale ? 'v' : 'v warn'}>{formatVoxelSize(project.scale)}</span>
          </div>
        ) : (
          <p className="empty-note">No project open.</p>
        )}
      </div>

      <div className="section">
        <div className="section-title">
          Project status
          <span className="minor">{cards.length === 0 ? 'HEALTHY' : 'ADVISORIES'}</span>
        </div>
        <div className="project-status">
          <StatusRow
            label="Project file"
            dot={status.saved ? 'dot' : 'dot warn'}
            value={status.saved ? 'Saved' : 'Unsaved changes'}
          />
          <StatusRow
            label="Backend"
            dot={backend === 'ready' ? 'dot' : backend === 'starting' ? 'dot progress' : 'dot down'}
            value={BACKEND_TEXT[backend]}
          />
          <StatusRow
            label="Working copy"
            dot={working ? 'dot progress' : 'dot'}
            value={
              working
                ? `${working.kind === 'proxy' ? 'Proxy' : 'Bricks'} · ${formatPercent(working.progress)}`
                : 'Ready'
            }
          />
          <div className="project-status-row">
            <span className="project-status-label">Pending writes</span>
            <span className="project-status-value">{status.pendingWrites}</span>
          </div>
        </div>
      </div>

      {cards.length > 0 ? (
        <div className="advisory-cards">
          {cards.map((card) => (
            <div
              className={card.tone === 'info' ? 'warning-card info' : 'warning-card'}
              key={card.id}
            >
              <AlertIcon />
              <div>
                <strong>{card.title}</strong>
                <br />
                {card.body}
              </div>
            </div>
          ))}
        </div>
      ) : null}
    </section>
  )
}

const AXIS_LABELS: Record<keyof AxisScale, string> = { z: 'Z', y: 'Y', x: 'X' }

function DisplayScaleSection() {
  const axisScale = useNav((s) => s.axisScale)
  const setAxisScale = useNav((s) => s.setAxisScale)
  const resetAxisScale = useNav((s) => s.resetAxisScale)

  return (
    <div className="section">
      <div className="section-title">
        Display scale
        <span className="minor">DISPLAY ONLY</span>
      </div>
      {AXIS_SCALE_KEYS.map((axis) => (
        <div className="scale-row" key={axis}>
          <span>{AXIS_LABELS[axis]}</span>
          <input
            type="range"
            aria-label={`${AXIS_LABELS[axis]} display scale`}
            min={AXIS_SCALE_MIN}
            max={AXIS_SCALE_MAX}
            step={0.1}
            value={axisScale[axis]}
            onChange={(e) => setAxisScale({ [axis]: Number(e.target.value) })}
          />
          <AxisScaleInput axis={axis} value={axisScale[axis]} onCommit={setAxisScale} />
        </div>
      ))}
      <div className="panel-actions">
        <button
          type="button"
          className="small-button"
          disabled={isPhysicalScale(axisScale)}
          onClick={resetAxisScale}
        >
          Reset to physical
        </button>
      </div>
    </div>
  )
}

interface AxisScaleInputProps {
  axis: keyof AxisScale
  value: number
  onCommit: (patch: Partial<AxisScale>) => void
}

function AxisScaleInput({ axis, value, onCommit }: AxisScaleInputProps) {
  const [draft, setDraft] = useState<string | null>(null)

  const commit = () => {
    if (draft === null) return
    const parsed = parseAxisScale(draft, AXIS_SCALE_MIN, AXIS_SCALE_MAX)
    setDraft(null)
    if (parsed !== null) onCommit({ [axis]: parsed })
  }

  return (
    <input
      type="number"
      aria-label={`${AXIS_LABELS[axis]} display scale value`}
      min={AXIS_SCALE_MIN}
      max={AXIS_SCALE_MAX}
      step={0.1}
      value={draft ?? Number(value.toFixed(2))}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === 'Enter') commit()
        if (e.key === 'Escape') setDraft(null)
      }}
    />
  )
}

interface SliderRowProps {
  label: string
  value: number
  min: number
  max: number
  readout: string
  onChange: (value: number) => void
}

function SliderRow({ label, value, min, max, readout, onChange }: SliderRowProps) {
  return (
    <div className="opacity-row">
      <span>{label}</span>
      <input
        type="range"
        aria-label={label}
        min={min}
        max={max}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <span>{readout}</span>
    </div>
  )
}

function StatusRow({ label, dot, value }: { label: string; dot: string; value: string }) {
  return (
    <div className="project-status-row">
      <span className="project-status-label">{label}</span>
      <span className="project-status-value">
        <i className={dot} />
        {value}
      </span>
    </div>
  )
}
