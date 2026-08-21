import type { ChannelState } from '@cellstudio/viewer'

export interface ChannelSquare {
  index: number
  name: string
  color: string
  /** White frame on the square. */
  visible: boolean
  /** Dot marker under the square — independent of visibility (spec: channel control bar). */
  active: boolean
}

export function channelSquares(
  channels: readonly ChannelState[],
  activeChannel: number,
): ChannelSquare[] {
  return channels.map((c, index) => ({
    index,
    name: c.name,
    color: c.color,
    visible: c.visible,
    active: index === activeChannel,
  }))
}

/** The all-channels square reads as satisfied only when nothing is hidden. */
export function allChannelsVisible(channels: readonly ChannelState[]): boolean {
  return channels.length > 0 && channels.every((c) => c.visible)
}

export function activeChannelOf(
  channels: readonly ChannelState[],
  activeChannel: number,
): ChannelState | null {
  return channels[activeChannel] ?? null
}

export function visibleChannelIndices(channels: readonly ChannelState[]): number[] {
  return channels.flatMap((c, i) => (c.visible ? [i] : []))
}

export function channelTag(index: number): string {
  return `C${index + 1}`
}

/** Palette offered in the settings popover, matching the prototype's swatches. */
export const DISPLAY_COLORS: readonly { color: string; name: string }[] = [
  { color: '#ff5c73', name: 'Red' },
  { color: '#52df83', name: 'Green' },
  { color: '#5ba7ff', name: 'Blue' },
  { color: '#d67cff', name: 'Magenta' },
  { color: '#ffb100', name: 'Amber' },
  { color: '#4be0d3', name: 'Cyan' },
]

export const GAMMA_MIN = 0.2
export const GAMMA_MAX = 3

export function dtypeMax(dtype: 'u8' | 'u16' | 'u32'): number {
  return dtype === 'u8' ? 255 : dtype === 'u16' ? 65535 : 4294967295
}
