import type { ChannelState } from '@cellstudio/viewer'
import { describe, expect, it } from 'vitest'
import { activeChannelOf, allChannelsVisible, channelSquares, channelTag } from './channels'

const channel = (name: string, color: string, visible: boolean): ChannelState => ({
  name,
  visible,
  window: [0, 65535],
  gamma: 1,
  color,
})

const channels: ChannelState[] = [
  channel('membrane', '#ff5c73', true),
  channel('nuclei', '#52df83', true),
  channel('marker', '#5ba7ff', false),
]

describe('channelSquares', () => {
  it('paints each square its display color and frames the visible ones', () => {
    const squares = channelSquares(channels, 0)
    expect(squares.map((s) => s.color)).toEqual(['#ff5c73', '#52df83', '#5ba7ff'])
    expect(squares.map((s) => s.visible)).toEqual([true, true, false])
    expect(squares.map((s) => s.name)).toEqual(['membrane', 'nuclei', 'marker'])
  })

  it('marks exactly one square active, independent of visibility', () => {
    const squares = channelSquares(channels, 2)
    expect(squares.map((s) => s.active)).toEqual([false, false, true])
    // The hidden channel is the active one: both states read at once.
    expect(squares[2]).toMatchObject({ active: true, visible: false })
    expect(activeChannelOf(channels, 2)?.name).toBe('marker')
  })

  it('keeps the active marker where it was when all channels are turned on', () => {
    const allOn = channels.map((c) => ({ ...c, visible: true }))
    const squares = channelSquares(allOn, 2)
    expect(squares.every((s) => s.visible)).toBe(true)
    expect(squares.map((s) => s.active)).toEqual([false, false, true])
  })

  it('reports an out-of-range active channel as no target', () => {
    expect(activeChannelOf(channels, 7)).toBeNull()
    expect(channelSquares(channels, 7).some((s) => s.active)).toBe(false)
  })
})

describe('allChannelsVisible', () => {
  it('is satisfied only when nothing is hidden', () => {
    expect(allChannelsVisible(channels)).toBe(false)
    expect(allChannelsVisible(channels.map((c) => ({ ...c, visible: true })))).toBe(true)
    expect(allChannelsVisible([])).toBe(false)
  })
})

describe('channelTag', () => {
  it('labels channels from one', () => {
    expect(channelTag(0)).toBe('C1')
    expect(channelTag(2)).toBe('C3')
  })
})
