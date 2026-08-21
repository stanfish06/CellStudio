import { VivLayerExtension } from '@hms-dbmi/viv'

/** Mirrors `@vivjs/constants`, which `@hms-dbmi/viv` does not re-export. */
const CHANNEL_INDEX = 'VIV_CHANNEL_INDEX'

export const GAMMA_MIN = 0.2
export const GAMMA_MAX = 3

export const clampGamma = (g: number): number =>
  Number.isFinite(g) ? Math.min(GAMMA_MAX, Math.max(GAMMA_MIN, g)) : 1

/**
 * Applies the contrast-limits ramp and then gamma. Defining
 * `fs:DECKGL_PROCESS_INTENSITY` suppresses `XRLayer`'s own ramp injection, so the ramp
 * has to live here rather than being layered on top of it.
 */
const gammaModule = {
  name: 'gammaModule',
  uniformTypes: { [`gamma${CHANNEL_INDEX}`]: 'f32' } as Record<string, 'f32'>,
  fs: `uniform gammaModuleUniforms {
  float gamma${CHANNEL_INDEX};
} gammaModule;

float apply_window_gamma(float intensity, vec2 limits, int channelIndex) {
  float gammas[NUM_CHANNELS] = float[NUM_CHANNELS](
    gammaModule.gamma${CHANNEL_INDEX},
  );
  float scaled = max(0., (intensity - limits[0]) / max(0.0005, limits[1] - limits[0]));
  return pow(min(1., scaled), max(0.01, gammas[channelIndex]));
}
`,
  inject: {
    'fs:DECKGL_PROCESS_INTENSITY': `
    intensity = apply_window_gamma(intensity, contrastLimits, channelIndex);
  `,
  },
}

/** What the extension reads off the layer it is attached to. */
interface GammaHost {
  props: { selections?: unknown[]; gammas?: number[] }
  getModels(): { shaderInputs: { setProps(props: Record<string, unknown>): void } }[]
}

export interface GammaExtensionProps {
  /** Per-channel exponent in selection order, 0.20–3.00; 1 is identity. */
  gammas: number[]
}

/**
 * Per-channel gamma on the image layers. Gamma is not a stock viv control, so it enters
 * through the same fragment hook viv uses for its contrast ramp.
 */
export class GammaExtension extends VivLayerExtension {
  static override extensionName = 'GammaExtension'
  static defaultProps = { gammas: { type: 'array', value: [], compare: true } }

  getVivShaderTemplates() {
    return { modules: [gammaModule] }
  }

  override updateState(
    params: Parameters<VivLayerExtension['updateState']>[0],
    extension: this,
  ): void {
    super.updateState(params, extension)
    const layer = this as unknown as GammaHost
    const count = layer.props.selections?.length ?? 0
    const gammas = layer.props.gammas ?? []
    const uniforms: Record<string, number> = {}
    for (let i = 0; i < count; i += 1) uniforms[`gamma${i}`] = clampGamma(gammas[i] ?? 1)
    for (const model of layer.getModels()) {
      model.shaderInputs.setProps({ gammaModule: uniforms })
    }
  }
}
