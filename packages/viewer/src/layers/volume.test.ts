import { describe, expect, it } from 'vitest'
import { XR3DLayer } from '@hms-dbmi/viv'
import { gamma3DExtension, packVolumeForViv, scaleTransform, volumeProps } from './volume'
import { vivLayer } from './viv'
import { channelsFor, devProject, layerProps, makeVolume } from '../test/data'
import type { VolumeBuffer } from '@cellstudio/api-client'

const ramp = (z: number, y: number, x: number): VolumeBuffer => {
  const data = new Uint16Array(z * y * x)
  for (let i = 0; i < data.length; i += 1) data[i] = i
  return { shape: [z, y, x], dtype: 'u16', level: 0, data: data.buffer }
}

describe('packVolumeForViv', () => {
  it('flips y within each z-plane, matching viv own volume assembly', () => {
    const packed = packVolumeForViv(ramp(2, 3, 2)) as Uint16Array
    // plane 0 rows [0,1] [2,3] [4,5] -> [4,5] [2,3] [0,1]
    expect([...packed.slice(0, 6)]).toEqual([4, 5, 2, 3, 0, 1])
    expect([...packed.slice(6)]).toEqual([10, 11, 8, 9, 6, 7])
  })

  it('is an involution, so the convention can be verified against a GPU', () => {
    const once = packVolumeForViv(ramp(2, 3, 2))
    const twice = packVolumeForViv({
      shape: [2, 3, 2],
      dtype: 'u16',
      level: 0,
      data: once.buffer as ArrayBuffer,
    })
    expect([...(twice as Uint16Array)]).toEqual([...new Uint16Array(ramp(2, 3, 2).data)])
  })

  it('can be turned off without copying', () => {
    const volume = ramp(1, 2, 2)
    expect([...(packVolumeForViv(volume, false) as Uint16Array)]).toEqual([0, 1, 2, 3])
  })
})

describe('scaleTransform', () => {
  it('turns texture dimensions into the world box viv scales the unit cube by', () => {
    const unit = [1, 1, 2.0 / 0.603] as const
    expect(scaleTransform(unit).transformPoint([256, 256, 3])).toEqual([
      256,
      256,
      3 * (2.0 / 0.603),
    ])
  })
})

describe('volumeProps', () => {
  const project = devProject()
  const channels = channelsFor(project).slice(0, 2)

  it('packs one texture per channel with matching window, color and gamma', () => {
    const props = volumeProps({
      id: 'v',
      channels: channels.map((channel, index) => ({
        channel,
        index,
        buffer: makeVolume(3, 256, 256),
      })),
      unit: [1, 1, 3.317],
      t: 12,
    })
    expect(props.channelData.data).toHaveLength(2)
    expect(props.channelData).toMatchObject({ width: 256, height: 256, depth: 3 })
    expect(props.contrastLimits).toEqual([channels[0]?.window, channels[1]?.window])
    expect(props.gammas).toEqual([1, 1])
    expect(props.selections).toEqual([
      { t: 12, c: 0 },
      { t: 12, c: 1 },
    ])
    expect(props.physicalSizeScalingMatrix.transformPoint([256, 256, 3])).toEqual([
      256,
      256,
      3 * 3.317,
    ])
  })

  it('builds an XR3DLayer that carries the channel data', () => {
    const props = volumeProps({
      id: 'v',
      channels: [{ channel: channels[0] as never, index: 0, buffer: makeVolume(3, 256, 256) }],
      unit: [1, 1, 1],
      t: 0,
    })
    const layer = layerProps(
      vivLayer(XR3DLayer, { ...props, extensions: [gamma3DExtension('mip')] }),
    )
    expect((layer.channelData as { data: unknown[] }).data).toHaveLength(1)
    expect(layer.selections).toHaveLength(1)
    expect(layer.extensions).toHaveLength(1)
  })
})

describe('gamma3DExtension', () => {
  it('keeps the blend mode viv 3D layer requires while adding gamma', () => {
    const additive = gamma3DExtension('additive') as {
      rendering?: { _RENDER?: string }
      getVivShaderTemplates(): { modules: { inject: Record<string, string> }[] }
    }
    expect(additive.rendering?._RENDER).toBeTruthy()
    expect(
      additive.getVivShaderTemplates().modules[0]?.inject['fs:DECKGL_PROCESS_INTENSITY'],
    ).toContain('apply_window_gamma_3d')
  })

  it('uses the same normalized ** gamma transfer the 2D path and the ui curve use', () => {
    const additive = gamma3DExtension('additive') as {
      getVivShaderTemplates(): { modules: { fs: string }[] }
    }
    const fs = additive.getVivShaderTemplates().modules[0]?.fs ?? ''
    expect(fs).toMatch(/pow\(min\(1\., scaled\), max\(0\.01, gammas\[channelIndex\]\)\)/)
  })

  it('gives each mode its own rendering body and reuses instances', () => {
    const mip = gamma3DExtension('mip') as { rendering: { _RENDER: string } }
    const min = gamma3DExtension('min') as { rendering: { _RENDER: string } }
    expect(mip.rendering._RENDER).toContain('max(')
    expect(min.rendering._RENDER).toContain('min(')
    expect(gamma3DExtension('mip')).toBe(mip)
  })
})
