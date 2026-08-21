import { ColorPalette3DExtensions } from '@hms-dbmi/viv'
import type { Dtype, VolumeBuffer } from '@cellstudio/api-client'
import type { ChannelState } from '../state/nav'
import type { WorldXYZ } from '../data/world'
import { clampGamma } from './gamma'
import { hexToRgb, vivDtype, type ChannelTexture } from './orthoPlane'

/** Mirrors `@vivjs/constants`, which `@hms-dbmi/viv` does not re-export. */
const CHANNEL_INDEX = 'VIV_CHANNEL_INDEX'

export type RenderingMode = 'additive' | 'mip' | 'min'

const typedArray = (dtype: Dtype, data: ArrayBuffer): ChannelTexture =>
  dtype === 'u8'
    ? new Uint8Array(data)
    : dtype === 'u16'
      ? new Uint16Array(data)
      : new Uint32Array(data)

const emptyLike = (source: ChannelTexture): ChannelTexture =>
  source instanceof Uint8Array
    ? new Uint8Array(source.length)
    : source instanceof Uint16Array
      ? new Uint16Array(source.length)
      : new Uint32Array(source.length)

/**
 * viv's own volume assembly writes each z-plane with y reversed, so a buffer that came
 * from `/volume` in raster order needs the same flip to land the right way up.
 */
export function packVolumeForViv(volume: VolumeBuffer, flipY = true): ChannelTexture {
  const [depth, height, width] = volume.shape
  const source = typedArray(volume.dtype, volume.data)
  if (!flipY) return source
  const out = emptyLike(source)
  for (let z = 0; z < depth; z += 1) {
    const base = z * height * width
    for (let y = 0; y < height; y += 1) {
      const from = base + y * width
      out.set(source.subarray(from, from + width), base + (height - 1 - y) * width)
    }
  }
  return out
}

/** viv only ever calls `transformPoint` on `physicalSizeScalingMatrix`. */
export const scaleTransform = (unit: WorldXYZ) => ({
  transformPoint: ([x, y, z]: readonly number[]): number[] => [
    (x ?? 0) * unit[0],
    (y ?? 0) * unit[1],
    (z ?? 0) * unit[2],
  ],
})

export interface VolumeChannel {
  buffer: VolumeBuffer
  channel: ChannelState
  index: number
}

export interface VolumeArgs {
  id: string
  channels: VolumeChannel[]
  /** World units per voxel at the volume's level, [x, y, z]. */
  unit: WorldXYZ
  t: number
  flipY?: boolean
}

export interface VolumeProps {
  id: string
  channelData: { data: ChannelTexture[]; width: number; height: number; depth: number }
  contrastLimits: [number, number][]
  channelsVisible: boolean[]
  colors: [number, number, number][]
  gammas: number[]
  selections: Record<string, number>[]
  dtype: 'Uint8' | 'Uint16' | 'Uint32'
  physicalSizeScalingMatrix: { transformPoint(p: readonly number[]): number[] }
  pickable: false
}

export function volumeProps(args: VolumeArgs): VolumeProps {
  const first = args.channels[0]
  if (!first) throw new Error('volumeProps needs at least one channel')
  const [depth, height, width] = first.buffer.shape
  return {
    id: args.id,
    channelData: {
      data: args.channels.map((c) => packVolumeForViv(c.buffer, args.flipY ?? true)),
      width,
      height,
      depth,
    },
    contrastLimits: args.channels.map((c) => [c.channel.window[0], c.channel.window[1]]),
    channelsVisible: args.channels.map((c) => c.channel.visible),
    colors: args.channels.map((c) => hexToRgb(c.channel.color)),
    gammas: args.channels.map((c) => clampGamma(c.channel.gamma)),
    selections: args.channels.map((c) => ({ t: args.t, c: c.index })),
    dtype: vivDtype(first.buffer.dtype),
    physicalSizeScalingMatrix: scaleTransform(args.unit),
    pickable: false,
  }
}

/**
 * Defining `fs:DECKGL_PROCESS_INTENSITY` suppresses `XR3DLayer`'s own contrast ramp, so
 * this module applies the ramp and then gamma, as the 2D path does.
 */
const gamma3dModule = {
  name: 'gammaModule',
  uniformTypes: { [`gamma${CHANNEL_INDEX}`]: 'f32' } as Record<string, 'f32'>,
  fs: `uniform gammaModuleUniforms {
  float gamma${CHANNEL_INDEX};
} gammaModule;

float apply_window_gamma_3d(float intensity, vec2 limits, int channelIndex) {
  float gammas[NUM_CHANNELS] = float[NUM_CHANNELS](
    gammaModule.gamma${CHANNEL_INDEX},
  );
  float scaled = max(0., (intensity - limits[0]) / max(0.0005, limits[1] - limits[0]));
  return pow(min(1., scaled), max(0.01, gammas[channelIndex]));
}
`,
  inject: {
    'fs:DECKGL_PROCESS_INTENSITY': `
    intensity = apply_window_gamma_3d(intensity, contrastLimits, channelIndex);
  `,
  },
}

interface GammaHost {
  props: { selections?: unknown[]; gammas?: number[] }
  getModels(): { shaderInputs: { setProps(props: Record<string, unknown>): void } }[]
}

type Extension3DBase = new (opts?: unknown) => object

const parentFor = (mode: RenderingMode): Extension3DBase =>
  (mode === 'mip'
    ? ColorPalette3DExtensions.MaximumIntensityProjectionExtension
    : mode === 'min'
      ? ColorPalette3DExtensions.MinimumIntensityProjectionExtension
      : ColorPalette3DExtensions.AdditiveBlendExtension) as unknown as Extension3DBase

const cache = new Map<RenderingMode, object>()

/**
 * A blend-mode extension with per-channel gamma folded in. It has to subclass viv's 3D
 * rendering extensions because `XR3DLayer` reads `extension.rendering` unguarded, so a
 * standalone gamma extension in that array would throw.
 */
export function gamma3DExtension(mode: RenderingMode): object {
  const cached = cache.get(mode)
  if (cached) return cached
  const Parent = parentFor(mode)
  class Gamma3DExtension extends Parent {
    static extensionName = `Gamma3D-${mode}`

    getVivShaderTemplates() {
      return { modules: [gamma3dModule] }
    }

    updateState(): void {
      const layer = this as unknown as GammaHost
      const count = layer.props.selections?.length ?? 0
      const gammas = layer.props.gammas ?? []
      const uniforms: Record<string, number> = {}
      for (let i = 0; i < count; i += 1) uniforms[`gamma${i}`] = clampGamma(gammas[i] ?? 1)
      for (const model of layer.getModels()) model.shaderInputs.setProps({ gammaModule: uniforms })
    }
  }
  const instance = new Gamma3DExtension()
  cache.set(mode, instance)
  return instance
}
