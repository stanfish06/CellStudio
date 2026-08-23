import { ColorPalette3DExtensions, XR3DLayer } from '@hms-dbmi/viv'
import type { VolumeBuffer } from '@cellstudio/api-client'
import type { WorldXYZ } from '../data/world'
import {
  LABEL_MODULE_NAME,
  PASS_LABEL_INTENSITY,
  clampLabelId,
  clampOpacity,
  labelShaderFs,
  labelUniformTypes,
  labelUniforms,
  type LabelHost,
} from './labelPalette'
import { vivDtype, type ChannelTexture } from './orthoPlane'
import { packVolumeForViv, scaleTransform } from './volume'

/** Mirrors `@vivjs/constants`, which `@hms-dbmi/viv` does not re-export. */
const CHANNEL_INDEX = 'VIV_CHANNEL_INDEX'

/**
 * `XR3DLayer`'s main applies `linear_to_srgb` to the colour it outputs, which the 2D path
 * does not — inverting it here is what makes the same id the same colour in both views.
 */
const labelVolumeModule = {
  name: LABEL_MODULE_NAME,
  uniformTypes: labelUniformTypes,
  fs: `${labelShaderFs(LABEL_MODULE_NAME)}
float label_srgb_to_linear(float x) {
  return x <= 0.04045 ? x / 12.92 : pow((x + 0.055) / 1.055, 2.4);
}

vec4 label_volume_color(vec4 rgba) {
  return vec4(
    label_srgb_to_linear(rgba.r),
    label_srgb_to_linear(rgba.g),
    label_srgb_to_linear(rgba.b),
    rgba.a
  );
}
`,
  inject: { 'fs:DECKGL_PROCESS_INTENSITY': PASS_LABEL_INTENSITY },
}

const _BEFORE_RENDER = `  vec4 labelHit = vec4(0.);
`

/**
 * First hit, not a blend: the ray stops at the first non-zero sample, so two ids never
 * average into a third cell's colour. viv expands any line carrying the channel
 * placeholder once per channel, so that line stays a self-contained statement.
 */
const _RENDER = `  if (labelHit.a == 0.) { labelHit = label_color(intensityValue${CHANNEL_INDEX}); }
  if (labelHit.a > 0.) { break; }
`

const _AFTER_RENDER = `  color = label_volume_color(labelHit);
`

type Extension3DBase = new (opts?: unknown) => object

const Base = ColorPalette3DExtensions.BaseExtension as unknown as Extension3DBase

let cached: object | undefined

/**
 * The label overlay's 3D rendering mode, beside viv's additive, MIP and min. It has to
 * subclass viv's 3D extension family because `XR3DLayer` reads `extension.rendering`
 * unguarded, so a bare `VivLayerExtension` in that array throws on the first frame.
 */
export function labelVolumeExtension(): object {
  if (cached) return cached
  class LabelVolumeExtension extends Base {
    static extensionName = 'LabelVolumeExtension'
    rendering = { _BEFORE_RENDER, _RENDER, _AFTER_RENDER }

    getVivShaderTemplates() {
      return { modules: [labelVolumeModule] }
    }

    updateState(): void {
      const layer = this as unknown as LabelHost
      const uniforms = labelUniforms(layer.props)
      for (const model of layer.getModels()) {
        model.shaderInputs.setProps({ [LABEL_MODULE_NAME]: uniforms })
      }
    }
  }
  const instance = new LabelVolumeExtension()
  cached = instance
  return instance
}

export interface LabelVolumeArgs {
  id: string
  volume: VolumeBuffer
  /** World units per voxel at the volume's level, [x, y, z]. */
  unit: WorldXYZ
  t: number
  /** Overlay opacity, applied to every non-zero id. */
  opacity: number
  /** Selected cell id, or 0. */
  selectedLabel?: number
  flipY?: boolean
}

export interface LabelVolumeProps {
  id: string
  channelData: { data: ChannelTexture[]; width: number; height: number; depth: number }
  contrastLimits: [number, number][]
  channelsVisible: boolean[]
  colors: [number, number, number][]
  selections: Record<string, number>[]
  dtype: 'Uint8' | 'Uint16' | 'Uint32'
  physicalSizeScalingMatrix: { transformPoint(p: readonly number[]): number[] }
  pickable: false
  selectedLabel: number
  labelOpacity: number
}

/** Props for the label volume drawn beside the image volume, at the same planned level. */
export function labelVolumeProps(args: LabelVolumeArgs): LabelVolumeProps {
  const [depth, height, width] = args.volume.shape
  return {
    id: args.id,
    channelData: {
      data: [packVolumeForViv(args.volume, args.flipY ?? true)],
      width,
      height,
      depth,
    },
    // Unused — the ramp is suppressed — but `XR3DLayer` pads both arrays on every update.
    contrastLimits: [[0, 1]],
    channelsVisible: [true],
    colors: [[255, 255, 255]],
    selections: [{ t: args.t, c: 0 }],
    dtype: vivDtype(args.volume.dtype),
    physicalSizeScalingMatrix: scaleTransform(args.unit),
    pickable: false,
    selectedLabel: clampLabelId(args.selectedLabel ?? 0),
    labelOpacity: clampOpacity(args.opacity),
  }
}

type Texture3DLayer = new (...args: never[]) => {
  context: { device: { createTexture(opts: Record<string, unknown>): unknown } }
}

const Volume3DBase = XR3DLayer as unknown as Texture3DLayer

/**
 * `XR3DLayer` hardcodes a linear 3D sampler, which is right for intensity and wrong for
 * ids: the first-hit march stops at a cell's surface, where a blend of the id and the
 * background rounds to an unrelated number and every cell comes out the same colour. The
 * 2D path escapes this because `interpolation: 'nearest'` reaches viv's 2D attributes.
 */
export class LabelVolumeLayer extends Volume3DBase {
  static layerName = 'LabelVolumeLayer'

  dataToTexture(data: ChannelTexture, width: number, height: number, depth: number): unknown {
    return this.context.device.createTexture({
      width,
      height,
      depth,
      dimension: '3d',
      data: new Float32Array(data),
      format: 'r32float',
      mipmaps: false,
      sampler: {
        minFilter: 'nearest',
        magFilter: 'nearest',
        addressModeU: 'clamp-to-edge',
        addressModeV: 'clamp-to-edge',
        addressModeW: 'clamp-to-edge',
      },
    })
  }
}
