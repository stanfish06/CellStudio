import { describe, expect, it } from 'vitest'
import { XRLayer } from '@hms-dbmi/viv'
import {
  OrthoPlaneLayer,
  hexToRgb,
  orthoPlaneExtensions,
  orthoPlaneProps,
  splitPlaneChannels,
  vivDtype,
} from './orthoPlane'
import { vivLayer } from './viv'
import { channelsFor, devProject, layerProps } from '../test/data'
import type { PlaneBuffer } from '@cellstudio/api-client'

const packed = (): PlaneBuffer => {
  const data = new Uint16Array([1, 2, 3, 4, 5, 6, 10, 20, 30, 40, 50, 60])
  return { shape: [2, 3], channels: 2, dtype: 'u16', level: 0, data: data.buffer }
}

describe('packed ortho planes', () => {
  it('splits channel-major bytes into one view per channel', () => {
    const views = splitPlaneChannels(packed())
    expect(views).toHaveLength(2)
    expect([...(views[0] as Uint16Array)]).toEqual([1, 2, 3, 4, 5, 6])
    expect([...(views[1] as Uint16Array)]).toEqual([10, 20, 30, 40, 50, 60])
  })

  it('maps wire dtypes to viv sampler names', () => {
    expect(vivDtype('u8')).toBe('Uint8')
    expect(vivDtype('u16')).toBe('Uint16')
    expect(vivDtype('u32')).toBe('Uint32')
  })

  it('parses display colors', () => {
    expect(hexToRgb('#ff8000')).toEqual([255, 128, 0])
    expect(hexToRgb('0f0')).toEqual([0, 255, 0])
  })
})

describe('orthoPlaneProps', () => {
  const project = devProject()
  const channels = channelsFor(project).slice(0, 2)

  it('carries window, color and gamma per channel and sizes the quad in world units', () => {
    const props = orthoPlaneProps({
      id: 'xz',
      plane: packed(),
      bounds: [0, 9.95, 1024, 0],
      channels,
      selections: [
        { t: 3, c: 0, index: 512 },
        { t: 3, c: 1, index: 512 },
      ],
    })
    expect(props.contrastLimits).toEqual([channels[0]?.window, channels[1]?.window])
    expect(props.colors).toEqual([hexToRgb('#ff0000'), hexToRgb('#00ff00')])
    expect(props.gammas).toEqual([1, 1])
    expect(props.bounds).toEqual([0, 9.95, 1024, 0])
    expect(props.dtype).toBe('Uint16')
    expect(props.interpolation).toBe('nearest')
    expect(props.channelData.width).toBe(3)
    expect(props.channelData.height).toBe(2)
  })

  it('clamps gamma into the control range', () => {
    const wild = channels.map((c, i) => ({ ...c, gamma: i === 0 ? 12 : 0.001 }))
    expect(
      orthoPlaneProps({
        id: 'xz',
        plane: packed(),
        bounds: [0, 1, 1, 0],
        channels: wild,
        selections: [
          { t: 0, c: 0 },
          { t: 0, c: 1 },
        ],
      }).gammas,
    ).toEqual([3, 0.2])
  })
})

describe('OrthoPlaneLayer', () => {
  it('is an XRLayer, so it shares the multiscale fragment path', () => {
    expect(Object.getPrototypeOf(OrthoPlaneLayer)).toBe(XRLayer)
    expect(OrthoPlaneLayer.layerName).toBe('OrthoPlaneLayer')
  })

  it('constructs with the props the quad needs', () => {
    const project = devProject()
    const props = orthoPlaneProps({
      id: 'xz',
      plane: packed(),
      bounds: [0, 9.95, 1024, 0],
      channels: channelsFor(project).slice(0, 1),
      selections: [{ t: 0, c: 0 }],
    })
    const layer = layerProps(
      vivLayer(OrthoPlaneLayer, { ...props, extensions: orthoPlaneExtensions() }),
    )
    expect(layer.bounds).toEqual([0, 9.95, 1024, 0])
    expect(layer.dtype).toBe('Uint16')
    expect(layer.selections).toHaveLength(1)
    expect(layer.extensions).toHaveLength(2)
  })
})
