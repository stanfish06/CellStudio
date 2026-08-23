import { describe, expect, it, vi } from 'vitest'
import { ColorPalette3DExtensions, VivLayerExtension } from '@hms-dbmi/viv'
import { LabelVolumeLayer, labelVolumeExtension, labelVolumeProps } from './labelVolume'
import { LABEL_MODULE_NAME, labelShaderFs } from './labelPalette'
import type { VolumeBuffer } from '@cellstudio/api-client'

interface Rendering {
  _BEFORE_RENDER: string
  _RENDER: string
  _AFTER_RENDER: string
}

interface Extension3D {
  rendering: Rendering
  updateState(): void
  getVivShaderTemplates(): {
    modules: { name: string; fs: string; inject: Record<string, string> }[]
  }
}

const extension = labelVolumeExtension() as Extension3D

const volumeModule = () => extension.getVivShaderTemplates().modules[0]

const labelVolume = (ids: number[]): VolumeBuffer => ({
  shape: [ids.length, 1, 1],
  dtype: 'u32',
  level: 2,
  data: new Uint32Array(ids).buffer,
})

describe('labelVolumeExtension', () => {
  /**
   * `XR3DLayer` reads `extension.rendering` unguarded, so an extension that is not one of
   * viv's 3D rendering extensions throws on the first frame the overlay is on.
   */
  it('reaches viv 3D rendering family, so the layer finds a rendering body', () => {
    const Base = ColorPalette3DExtensions.BaseExtension as unknown as new () => object
    expect(extension instanceof Base).toBe(true)
    expect(extension instanceof VivLayerExtension).toBe(true)
    expect(extension.rendering._RENDER).toBeTruthy()
    expect(extension.rendering._BEFORE_RENDER).toBeTruthy()
    expect(extension.rendering._AFTER_RENDER).toBeTruthy()
  })

  it('reuses one instance', () => {
    expect(labelVolumeExtension()).toBe(extension)
  })

  it('terminates the ray at the first non-zero sample instead of accumulating', () => {
    const { _BEFORE_RENDER, _RENDER, _AFTER_RENDER } = extension.rendering
    expect(_BEFORE_RENDER).toContain('vec4 labelHit = vec4(0.);')
    expect(_RENDER).toContain('if (labelHit.a == 0.)')
    expect(_RENDER).toContain('break;')
    expect(_AFTER_RENDER).toContain('color = label_volume_color(labelHit);')
    // Nothing sums or blends two samples into a third cell colour.
    expect(_RENDER).not.toContain('+=')
  })

  /** viv expands every line carrying the channel placeholder once per channel. */
  it('keeps the channel-indexed line a self-contained statement', () => {
    const lines = extension.rendering._RENDER
      .split('\n')
      .filter((l) => l.includes('VIV_CHANNEL_INDEX'))
    expect(lines).toHaveLength(1)
    const line = lines[0] as string
    expect(line.split('{')).toHaveLength(line.split('}').length)
    expect(line.trimEnd().endsWith('}')).toBe(true)
  })

  it('suppresses viv contrast ramp and leaves colour to the rendering body', () => {
    const inject = volumeModule()?.inject ?? {}
    expect(Object.keys(inject)).toEqual(['fs:DECKGL_PROCESS_INTENSITY'])
    expect(inject['fs:DECKGL_PROCESS_INTENSITY']?.trim().length).toBeGreaterThan(0)
  })

  it('shares the 2D hash verbatim, so an id is one colour in both views', () => {
    expect(volumeModule()?.fs).toContain(labelShaderFs(LABEL_MODULE_NAME))
  })

  /** `XR3DLayer` applies `linear_to_srgb` to its output; the 2D path does not. */
  it('inverts the srgb encoding viv 3D main applies afterwards', () => {
    expect(volumeModule()?.fs).toContain('float label_srgb_to_linear(float x)')
    expect(volumeModule()?.fs).toContain('pow((x + 0.055) / 1.055, 2.4)')
  })

  it('pushes clamped uniforms into the model', () => {
    const setProps = vi.fn()
    const host = {
      props: { selectedLabel: 5, labelOpacity: -1 },
      getModels: () => [{ shaderInputs: { setProps } }],
    }
    extension.updateState.call(host as never)
    expect(setProps).toHaveBeenCalledWith({
      [LABEL_MODULE_NAME]: { selectedLabel: 5, labelOpacity: 0 },
    })
  })
})

describe('labelVolumeProps', () => {
  it('packs one u32 channel with the overlay opacity and selection', () => {
    const props = labelVolumeProps({
      id: 'labels-3d',
      volume: labelVolume([0, 7, 9]),
      unit: [1, 1, 3.317],
      t: 4,
      opacity: 0.36,
      selectedLabel: 7,
      flipY: false,
    })
    expect(props.dtype).toBe('Uint32')
    expect(props.channelData).toMatchObject({ width: 1, height: 1, depth: 3 })
    expect([...(props.channelData.data[0] as Uint32Array)]).toEqual([0, 7, 9])
    expect(props.selections).toEqual([{ t: 4, c: 0 }])
    expect(props.selectedLabel).toBe(7)
    expect(props.labelOpacity).toBe(0.36)
    expect(props.pickable).toBe(false)
    expect(props.physicalSizeScalingMatrix.transformPoint([1, 1, 3])).toEqual([1, 1, 3 * 3.317])
  })

  it('flips y per z-plane by default, as viv volume assembly does', () => {
    const props = labelVolumeProps({
      id: 'labels-3d',
      volume: {
        shape: [1, 2, 2],
        dtype: 'u32',
        level: 0,
        data: new Uint32Array([1, 2, 3, 4]).buffer,
      },
      unit: [1, 1, 1],
      t: 0,
      opacity: 1,
    })
    expect([...(props.channelData.data[0] as Uint32Array)]).toEqual([3, 4, 1, 2])
  })
})

describe('LabelVolumeLayer', () => {
  it('samples ids with nearest, because a blended id is a different cell', () => {
    const created: Record<string, unknown>[] = []
    const layer = new LabelVolumeLayer() as unknown as {
      context: { device: { createTexture(o: Record<string, unknown>): unknown } }
      dataToTexture(d: Uint32Array, w: number, h: number, z: number): unknown
    }
    layer.context = {
      device: {
        createTexture: (o) => {
          created.push(o)
          return o
        },
      },
    }
    layer.dataToTexture(new Uint32Array([0, 7, 4096, 0]), 2, 2, 1)
    const sampler = created[0]?.sampler as Record<string, string>
    expect(sampler.minFilter).toBe('nearest')
    expect(sampler.magFilter).toBe('nearest')
    // ids must survive the upload exactly, so the hash sees the cell it belongs to
    expect(Array.from(created[0]?.data as Float32Array)).toEqual([0, 7, 4096, 0])
  })
})
