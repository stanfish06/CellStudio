import type { Dtype, Histogram } from '@cellstudio/api-client'
import { useNav } from '@cellstudio/viewer'
import { useRef } from 'react'
import { DISPLAY_COLORS, GAMMA_MAX, GAMMA_MIN, activeChannelOf, dtypeMax } from '../lib/channels'
import { formatCompact, formatDtype } from '../lib/format'
import { HIST_HEIGHT, HIST_WIDTH, histogramGeometry } from '../lib/histogram'
import { cssVars } from '../types'

const GAMMA_STEPS = 100

export interface ChannelSettingsPopoverProps {
  /** From `GET /histogram` for the active channel; null while a fresh one is in flight. */
  histogram: Histogram | null
  dtype: Dtype | null
  onClose: () => void
}

export function ChannelSettingsPopover({ histogram, dtype, onClose }: ChannelSettingsPopoverProps) {
  const channels = useNav((s) => s.channels)
  const activeChannel = useNav((s) => s.activeChannel)
  const setChannel = useNav((s) => s.setChannel)
  const previous = useRef<{ channel: number; hist: Histogram } | null>(null)
  if (histogram) previous.current = { channel: activeChannel, hist: histogram }
  const channel = activeChannelOf(channels, activeChannel)
  if (!channel) return null

  // Hold the last distribution while a fresh one loads, keyed by channel so the plot never
  // shows another channel's data.
  const shown =
    histogram ?? (previous.current?.channel === activeChannel ? previous.current.hist : null)
  const max = dtypeMax(dtype ?? 'u16')
  const [windowMin, windowMax] = channel.window
  const geometry = histogramGeometry({
    hist: shown,
    domain: [0, max],
    window: channel.window,
    gamma: channel.gamma,
  })

  return (
    <div className="settings-popover" role="dialog" aria-label="Channel settings">
      <div className="popover-head">
        <span>
          <i className="active-swatch" style={cssVars({ '--swatch': channel.color })} />
          Channel {activeChannel + 1} · {channel.name}
        </span>
        <button type="button" className="icon-button" aria-label="Close settings" onClick={onClose}>
          ×
        </button>
      </div>

      <div className="histogram-block">
        <div className="histogram-head">
          <span>Intensity distribution</span>
          <span>
            {dtype ? formatDtype(dtype) : ''}
            {shown?.sampled === true ? ' · sampled' : ''}
            {histogram === null && shown !== null ? ' · updating' : ''}
          </span>
        </div>
        <svg
          className="histogram"
          viewBox={`0 0 ${HIST_WIDTH} ${HIST_HEIGHT}`}
          preserveAspectRatio="none"
          role="img"
          aria-label="Channel intensity histogram with display window"
        >
          {geometry.fill ? <path className="hist-fill" d={geometry.fill} /> : null}
          {geometry.outline ? <path className="hist-line" d={geometry.outline} /> : null}
          <rect
            className="hist-window"
            x={geometry.window.x}
            width={geometry.window.width}
            y={0}
            height={HIST_HEIGHT}
          />
          <line
            className="hist-limit"
            x1={geometry.minX}
            x2={geometry.minX}
            y1={0}
            y2={HIST_HEIGHT}
          />
          <line
            className="hist-limit"
            x1={geometry.maxX}
            x2={geometry.maxX}
            y1={0}
            y2={HIST_HEIGHT}
          />
          <path className="lut-curve" d={geometry.curve} />
        </svg>
        <div className="histogram-scale">
          {geometry.ticks.map((tick, i) => (
            <span key={i}>{tick}</span>
          ))}
        </div>
      </div>

      <div className="control-row">
        <label htmlFor="channel-window-min">Window min</label>
        <input
          id="channel-window-min"
          type="range"
          min={0}
          max={max}
          value={windowMin}
          onChange={(e) =>
            setChannel(activeChannel, { window: [Number(e.target.value), windowMax] })
          }
        />
        <output>{formatCompact(windowMin)}</output>
      </div>

      <div className="control-row">
        <label htmlFor="channel-window-max">Window max</label>
        <input
          id="channel-window-max"
          type="range"
          min={0}
          max={max}
          value={windowMax}
          onChange={(e) =>
            setChannel(activeChannel, { window: [windowMin, Number(e.target.value)] })
          }
        />
        <output>{formatCompact(windowMax)}</output>
      </div>

      <div className="control-row">
        <label htmlFor="channel-gamma">Gamma</label>
        <input
          id="channel-gamma"
          type="range"
          min={GAMMA_MIN * GAMMA_STEPS}
          max={GAMMA_MAX * GAMMA_STEPS}
          value={Math.round(channel.gamma * GAMMA_STEPS)}
          onChange={(e) =>
            setChannel(activeChannel, { gamma: Number(e.target.value) / GAMMA_STEPS })
          }
        />
        <output>{channel.gamma.toFixed(2)}</output>
      </div>

      <div className="control-row">
        <span>Display color</span>
        <div className="color-picks">
          {DISPLAY_COLORS.map((swatch) => (
            <button
              key={swatch.color}
              type="button"
              className={swatch.color === channel.color ? 'color-pick selected' : 'color-pick'}
              style={cssVars({ '--c': swatch.color })}
              aria-label={swatch.name}
              aria-pressed={swatch.color === channel.color}
              onClick={() => setChannel(activeChannel, { color: swatch.color })}
            />
          ))}
        </div>
        <span />
      </div>
    </div>
  )
}
