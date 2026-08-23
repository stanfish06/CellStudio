import { describe, expect, it, vi } from 'vitest'
import {
  LABEL_MODULE_NAME,
  LABEL_PALETTE,
  LABEL_PALETTE_SIZE,
  LabelPaletteExtension,
  MAX_LABEL_ID,
  MIN_LABEL_LIGHTNESS,
  clampLabelId,
  labelColor,
  labelPlaneProps,
} from './labelPalette'
import { srgbToLab } from './palette'
import type { PlaneBuffer } from '@cellstudio/api-client'

interface ShaderModule {
  name: string
  uniformTypes: Record<string, string>
  fs: string
  inject: Record<string, string>
}

const extension = new LabelPaletteExtension()

const paletteModule = (): ShaderModule =>
  (extension.getVivShaderTemplates().modules as unknown as ShaderModule[])[0] as ShaderModule

const labelPlane = (ids: number[]): PlaneBuffer => ({
  shape: [1, ids.length],
  channels: 1,
  dtype: 'u32',
  level: 0,
  data: new Uint32Array(ids).buffer,
})

describe('clampLabelId', () => {
  it('caps at the 2^24 viv float path can distinguish and rejects nonsense', () => {
    expect(clampLabelId(1)).toBe(1)
    expect(clampLabelId(MAX_LABEL_ID)).toBe(MAX_LABEL_ID)
    expect(clampLabelId(MAX_LABEL_ID + 5)).toBe(MAX_LABEL_ID)
    expect(clampLabelId(-3)).toBe(0)
    expect(clampLabelId(Number.NaN)).toBe(0)
    expect(clampLabelId(7.8)).toBe(7)
  })
})

describe('labelColor', () => {
  /** Hard-coded so a change to the hash shows up as a diff: colours are persisted state. */
  it('is stable across sessions — a pure function of the id', () => {
    expect(labelColor(1)).toEqual([255, 0, 255])
    expect(labelColor(2)).toEqual([255, 0, 0])
    expect(labelColor(3)).toEqual([0, 255, 255])
    expect(labelColor(42)).toEqual(labelColor(42 + LABEL_PALETTE_SIZE))
    expect(labelColor(MAX_LABEL_ID)).toEqual(
      labelColor(((MAX_LABEL_ID - 1) % LABEL_PALETTE_SIZE) + 1),
    )
  })

  it('wraps past the end of the palette rather than running out', () => {
    expect(labelColor(LABEL_PALETTE_SIZE + 1)).toEqual(labelColor(1))
    expect(labelColor(LABEL_PALETTE_SIZE)).toEqual(labelColor(2 * LABEL_PALETTE_SIZE))
  })

  it('avoids the colours it was told the labels sit on', () => {
    // the signal is green and the background dark, so neither may be a label colour
    for (const entry of LABEL_PALETTE) {
      expect(entry).not.toEqual([0, 1, 0])
      expect(entry).not.toEqual([0, 0, 0])
    }
  })

  it('gives consecutive ids different entries, which neighbouring cells usually are', () => {
    const seen = new Set(
      Array.from({ length: LABEL_PALETTE_SIZE }, (_, i) => labelColor(i + 1).join(',')),
    )
    expect(seen.size).toBe(LABEL_PALETTE_SIZE)
  })

  it('is deterministic, with no per-session state', () => {
    const first = Array.from({ length: 64 }, (_, i) => labelColor(i + 1))
    const second = Array.from({ length: 64 }, (_, i) => labelColor(i + 1))
    expect(second).toEqual(first)
  })

  it('maps background to black, which the shader turns into alpha 0', () => {
    expect(labelColor(0)).toEqual([0, 0, 0])
    expect(paletteModule().fs).toContain('if (id == 0u) return vec4(0.);')
  })

  it('separates consecutive ids, so neighbouring cells never share a colour', () => {
    const deltas: number[] = []
    for (let id = 1; id < 512; id += 1) {
      const a = labelColor(id)
      const b = labelColor(id + 1)
      deltas.push(Math.max(...a.map((v, i) => Math.abs(v - (b[i] as number)))))
    }
    expect(Math.min(...deltas)).toBeGreaterThan(0)
    expect(deltas.filter((d) => d >= 32).length / deltas.length).toBeGreaterThan(0.95)
  })

  it('keeps every id light enough to read over the image', () => {
    for (const entry of LABEL_PALETTE) {
      expect(srgbToLab(entry)[0]).toBeGreaterThanOrEqual(MIN_LABEL_LIGHTNESS)
    }
  })
})

describe('LabelPaletteExtension shader module', () => {
  /**
   * The trap: `DECKGL_PROCESS_INTENSITY` is what suppresses viv's contrast ramp, and viv
   * detects it by truthiness. Defining `DECKGL_MUTATE_COLOR` alone would hand the colour
   * hook an id already windowed into 0..1, making the hash meaningless.
   */
  it('defines both hooks, and the intensity body is non-empty', () => {
    const inject = paletteModule().inject
    expect(Object.keys(inject).sort()).toEqual([
      'fs:DECKGL_MUTATE_COLOR',
      'fs:DECKGL_PROCESS_INTENSITY',
    ])
    expect(inject['fs:DECKGL_PROCESS_INTENSITY']?.trim().length).toBeGreaterThan(0)
    expect(inject['fs:DECKGL_PROCESS_INTENSITY']).not.toContain('contrastLimits')
  })

  it('colours from the single label channel', () => {
    expect(paletteModule().inject['fs:DECKGL_MUTATE_COLOR']).toContain(
      'rgba = label_color(intensity[0]);',
    )
  })

  it('gives the selected id a distinct highlight from a uniform', () => {
    const fs = paletteModule().fs
    expect(fs).toContain(`if (id == ${LABEL_MODULE_NAME}.selectedLabel)`)
    expect(fs).toContain(`mix(rgb, vec3(1.), 0.5)`)
    expect(fs).toContain(`vec4(rgb, ${LABEL_MODULE_NAME}.labelOpacity)`)
  })

  it('declares the uniform block the module name binds to', () => {
    const module = paletteModule()
    expect(module.name).toBe(LABEL_MODULE_NAME)
    expect(Object.keys(module.uniformTypes)).toEqual(['selectedLabel', 'labelOpacity'])
    expect(module.fs).toContain(`uniform ${LABEL_MODULE_NAME}Uniforms {`)
    expect(module.fs).toContain(`} ${LABEL_MODULE_NAME};`)
  })

  it('bakes the palette and indexes it by integer id, not through the contrast ramp', () => {
    const fs = paletteModule().fs
    expect(fs).toContain(`const vec3 LABEL_PALETTE[${LABEL_PALETTE_SIZE}]`)
    expect(fs).toContain(`LABEL_PALETTE[(id - 1u) % ${LABEL_PALETTE_SIZE}u]`)
    expect(fs).toContain('uint id = uint(max(0., value) + 0.5);')
    // the table the shader compiles is the one the CPU hands the inspector
    const first = LABEL_PALETTE[0] as readonly number[]
    expect(fs).toContain(`vec3(${first.map((c) => c.toFixed(6)).join(', ')})`)
  })

  it('pushes clamped uniforms into the model', () => {
    const setProps = vi.fn()
    const host = {
      props: { selectedLabel: 12.7, labelOpacity: 4 },
      getModels: () => [{ shaderInputs: { setProps } }],
    }
    LabelPaletteExtension.prototype.updateState.call(
      host as never,
      {} as never,
      extension as unknown as LabelPaletteExtension,
    )
    expect(setProps).toHaveBeenCalledWith({
      [LABEL_MODULE_NAME]: { selectedLabel: 12, labelOpacity: 1 },
    })
  })

  it('defaults to no selection and full alpha', () => {
    const setProps = vi.fn()
    const host = { props: {}, getModels: () => [{ shaderInputs: { setProps } }] }
    LabelPaletteExtension.prototype.updateState.call(
      host as never,
      {} as never,
      extension as unknown as LabelPaletteExtension,
    )
    expect(setProps).toHaveBeenCalledWith({
      [LABEL_MODULE_NAME]: { selectedLabel: 0, labelOpacity: 1 },
    })
  })
})

describe('labelPlaneProps', () => {
  it('carries one u32 channel and the overlay opacity as a uniform', () => {
    const props = labelPlaneProps({
      id: 'labels-xy',
      plane: labelPlane([0, 1, 9]),
      bounds: [0, 4, 3, 0],
      opacity: 0.36,
      selectedLabel: 9,
    })
    expect(props.dtype).toBe('Uint32')
    expect(props.channelData.data).toHaveLength(1)
    expect([...(props.channelData.data[0] as Uint32Array)]).toEqual([0, 1, 9])
    expect(props.selections).toEqual([{ c: 0 }])
    expect(props.selectedLabel).toBe(9)
    expect(props.labelOpacity).toBe(0.36)
  })

  /** deck's own opacity would multiply the shader alpha; the uniform is the one control. */
  it('leaves layer opacity at 1', () => {
    const props = labelPlaneProps({
      id: 'labels-xy',
      plane: labelPlane([1]),
      bounds: [0, 1, 1, 0],
      opacity: 0.5,
    })
    expect(props.opacity).toBe(1)
    expect(props.selectedLabel).toBe(0)
  })

  /** Picks have to reach the image plane below — label selection goes through `/pixel`. */
  it('is not pickable and never interpolates ids', () => {
    const props = labelPlaneProps({
      id: 'labels-xy',
      plane: labelPlane([1]),
      bounds: [0, 1, 1, 0],
      opacity: 1,
    })
    expect(props.pickable).toBe(false)
    expect(props.interpolation).toBe('nearest')
  })
})
