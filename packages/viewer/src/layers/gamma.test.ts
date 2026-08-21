import { describe, expect, it, vi } from 'vitest'
import { GAMMA_MAX, GAMMA_MIN, GammaExtension, clampGamma } from './gamma'

interface ShaderModule {
  name: string
  uniformTypes: Record<string, string>
  fs: string
  inject: Record<string, string>
}

const modulesOf = (extension: GammaExtension): ShaderModule[] =>
  extension.getVivShaderTemplates().modules as unknown as ShaderModule[]

describe('gamma clamping', () => {
  it('holds the control range and treats nonsense as identity', () => {
    expect(clampGamma(1)).toBe(1)
    expect(clampGamma(0.01)).toBe(GAMMA_MIN)
    expect(clampGamma(99)).toBe(GAMMA_MAX)
    expect(clampGamma(Number.NaN)).toBe(1)
  })
})

describe('GammaExtension shader module', () => {
  const extension = new GammaExtension()

  it('defines the intensity hook, which is what suppresses viv default ramp', () => {
    const [module] = modulesOf(extension)
    expect(module?.inject['fs:DECKGL_PROCESS_INTENSITY']).toContain('apply_window_gamma')
  })

  it('applies the contrast ramp itself, since defining the hook replaces it', () => {
    const [module] = modulesOf(extension)
    expect(module?.fs).toContain('limits[0]')
    expect(module?.fs).toContain('limits[1]')
  })

  it('raises the normalized value to the gamma exponent (napari convention)', () => {
    const [module] = modulesOf(extension)
    expect(module?.fs).toMatch(/pow\(min\(1\., scaled\), max\(0\.01, gammas\[channelIndex\]\)\)/)
  })

  /**
   * The curve the channel popover draws is `lutTransfer` in `@cellstudio/ui`:
   * `normalized ** gamma`, after windowing and before the channel color. Injecting only
   * into the intensity hook is what puts it there — viv's fragment shader runs
   * `DECKGL_PROCESS_INTENSITY` per channel and only then hands the intensities to
   * `DECKGL_MUTATE_COLOR`, which is where the color palette multiplies in.
   */
  it('injects into the intensity hook alone, so the color still comes after', () => {
    const [module] = modulesOf(extension)
    expect(Object.keys(module?.inject ?? {})).toEqual(['fs:DECKGL_PROCESS_INTENSITY'])
    const fs = module?.fs ?? ''
    expect(fs.indexOf('float scaled =')).toBeLessThan(fs.indexOf('return pow('))
  })

  it('declares one per-channel uniform, expanded by viv channel placeholder', () => {
    const [module] = modulesOf(extension)
    expect(Object.keys(module?.uniformTypes ?? {})).toEqual(['gammaVIV_CHANNEL_INDEX'])
    expect(module?.fs).toContain('float gammaVIV_CHANNEL_INDEX;')
    expect(module?.fs).toContain('gammaModule.gammaVIV_CHANNEL_INDEX,')
  })

  it('pushes one clamped uniform per selection into the model', () => {
    const setProps = vi.fn()
    const host = {
      props: { selections: [{}, {}], gammas: [0.5, 40] },
      getModels: () => [{ shaderInputs: { setProps } }],
    }
    GammaExtension.prototype.updateState.call(
      host as never,
      {} as never,
      extension as unknown as GammaExtension,
    )
    expect(setProps).toHaveBeenCalledWith({ gammaModule: { gamma0: 0.5, gamma1: GAMMA_MAX } })
  })

  it('defaults a channel without a gamma to identity', () => {
    const setProps = vi.fn()
    const host = { props: { selections: [{}] }, getModels: () => [{ shaderInputs: { setProps } }] }
    GammaExtension.prototype.updateState.call(
      host as never,
      {} as never,
      extension as unknown as GammaExtension,
    )
    expect(setProps).toHaveBeenCalledWith({ gammaModule: { gamma0: 1 } })
  })
})
