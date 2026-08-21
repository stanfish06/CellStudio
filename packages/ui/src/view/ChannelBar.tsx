import type { Dtype, Histogram } from '@cellstudio/api-client'
import { useNav } from '@cellstudio/viewer'
import { CheckIcon, GearIcon } from '../icons'
import { allChannelsVisible, channelSquares } from '../lib/channels'
import { VIEW_KEYS, VIEW_LABELS } from '../lib/keymap'
import { cssVars } from '../types'
import { ChannelSettingsPopover } from './ChannelSettingsPopover'

export interface ChannelBarProps {
  histogram: Histogram | null
  dtype: Dtype | null
  settingsOpen: boolean
  onSettingsToggle: () => void
  onSettingsClose: () => void
}

export function ChannelBar({
  histogram,
  dtype,
  settingsOpen,
  onSettingsToggle,
  onSettingsClose,
}: ChannelBarProps) {
  const channels = useNav((s) => s.channels)
  const activeChannel = useNav((s) => s.activeChannel)
  const toggleChannel = useNav((s) => s.toggleChannel)
  const showAllChannels = useNav((s) => s.showAllChannels)
  const activeView = useNav((s) => s.activeView)
  const setActiveView = useNav((s) => s.setActiveView)
  const overlays = useNav((s) => s.overlays)
  const setOverlays = useNav((s) => s.setOverlays)

  const squares = channelSquares(channels, activeChannel)

  return (
    <div className="channelbar">
      <span className="channel-label">Channels</span>
      {squares.map((square) => (
        <button
          key={square.index}
          type="button"
          className={`channel${square.visible ? ' visible' : ''}${square.active ? ' active-channel' : ''}`}
          style={cssVars({ '--swatch': square.color })}
          title={`${square.name} — click to toggle visibility`}
          aria-label={`Channel ${square.index + 1} ${square.name}, ${square.visible ? 'visible' : 'hidden'}${square.active ? ', active' : ''}`}
          aria-pressed={square.visible}
          onClick={() => toggleChannel(square.index)}
        >
          <span className="swatch" />
        </button>
      ))}
      <button
        type="button"
        className={`channel all-channels${allChannelsVisible(channels) ? ' visible' : ''}`}
        aria-label="Show all channels"
        title="Show all channels"
        onClick={showAllChannels}
      >
        <span className="swatch" />
      </button>
      <button
        type="button"
        className={settingsOpen ? 'icon-button active' : 'icon-button'}
        aria-label="Active channel settings"
        aria-expanded={settingsOpen}
        title="Channel settings"
        onClick={onSettingsToggle}
      >
        <GearIcon />
      </button>
      {settingsOpen ? (
        <ChannelSettingsPopover histogram={histogram} dtype={dtype} onClose={onSettingsClose} />
      ) : null}

      <div className="view-switcher" aria-label="View orientation">
        {VIEW_KEYS.map((view, i) => (
          <button
            key={view}
            type="button"
            className={view === activeView ? 'view-button active' : 'view-button'}
            aria-pressed={view === activeView}
            onClick={() => setActiveView(view)}
          >
            {VIEW_LABELS[view]} <span>{i + 1}</span>
          </button>
        ))}
      </div>

      <div className="overlay-controls">
        <OverlayCheck
          id="overlay-labels"
          label="Segmentation"
          checked={overlays.labels.on}
          onChange={(on) => setOverlays({ labels: { ...overlays.labels, on } })}
        />
        <OverlayCheck
          id="overlay-tracks"
          label="Tracks"
          checked={overlays.tracks.on}
          onChange={(on) => setOverlays({ tracks: { ...overlays.tracks, on } })}
        />
      </div>
    </div>
  )
}

interface OverlayCheckProps {
  id: string
  label: string
  checked: boolean
  onChange: (checked: boolean) => void
}

function OverlayCheck({ id, label, checked, onChange }: OverlayCheckProps) {
  return (
    <label className="overlay-check" htmlFor={id}>
      <input
        id={id}
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="check-box">
        <CheckIcon />
      </span>
      <span>{label}</span>
    </label>
  )
}
