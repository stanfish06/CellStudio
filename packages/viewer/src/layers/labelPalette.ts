import { VivLayerExtension } from '@hms-dbmi/viv'
import type { PlaneBuffer } from '@cellstudio/api-client'
import { splitPlaneChannels, vivDtype, type ChannelTexture } from './orthoPlane'
import { distinguishableColors, type Rgb01 } from './palette'
import type { Rgb } from './tracks'

/**
 * viv hands the fragment shader a float, so ids are distinguishable only to 2^24. The
 * server refuses to allocate past this; an adopted store may still exceed it (design M14).
 */
export const MAX_LABEL_ID = 0xffffff

export const clampLabelId = (id: number): number =>
  Number.isFinite(id) && id > 0 ? Math.min(MAX_LABEL_ID, Math.floor(id)) : 0

export const clampOpacity = (o: number): number =>
  Number.isFinite(o) ? Math.min(1, Math.max(0, o)) : 1

/**
 * Entries in the baked palette; consecutive ids take consecutive, well-separated entries,
 * and ids past the end wrap. Sized against the overlay, not the algorithm's 9000 ceiling:
 * the greedy pick separates 1024 colours by ΔE 7 and 2048 by 5.5, and a fill drawn at
 * ~36% opacity scales that difference down by about the same factor, so more entries buy
 * headroom the eye cannot use.
 */
export const LABEL_PALETTE_SIZE = 1024

/** What the labels sit on: the green the signal is brightest in, and the dark it sits in. */
const PALETTE_BACKGROUND: Rgb01[] = [
  [0, 1, 0],
  [0, 0, 0],
]

/** Lab lightness floor for a palette entry. */
export const MIN_LABEL_LIGHTNESS = 45

/**
 * Built once. The algorithm is deterministic and its sequence does not depend on the count,
 * so a cell keeps its colour across sessions and between the 2D and 3D overlays.
 */
export const LABEL_PALETTE: readonly Rgb01[] = distinguishableColors(
  LABEL_PALETTE_SIZE,
  PALETTE_BACKGROUND,
  // a label has to stay legible over a dark image at overlay opacity
  MIN_LABEL_LIGHTNESS,
)

/** Id 0 is background; ids 1..n take palette entries 0..n-1 before wrapping. */
const paletteIndex = (label: number): number => (label - 1) % LABEL_PALETTE_SIZE

/** Label id to colour, in the same order the shader's baked table uses. */
export function labelColor(id: number): Rgb {
  const label = clampLabelId(id)
  if (label === 0) return [0, 0, 0]
  const [r, g, b] = LABEL_PALETTE[paletteIndex(label)] as Rgb01
  return [Math.round(r * 255), Math.round(g * 255), Math.round(b * 255)]
}

export const LABEL_MODULE_NAME = 'labelPaletteModule'

export const labelUniformTypes: Record<string, 'u32' | 'f32'> = {
  selectedLabel: 'u32',
  labelOpacity: 'f32',
}

const paletteGlsl = (): string =>
  LABEL_PALETTE.map(
    ([r, g, b]) => `  vec3(${r.toFixed(6)}, ${g.toFixed(6)}, ${b.toFixed(6)})`,
  ).join(',\n')

/** The shared palette, keyed to the uniform block of whichever module declares it. */
export const labelShaderFs = (moduleName: string): string => `uniform ${moduleName}Uniforms {
  uint selectedLabel;
  float labelOpacity;
} ${moduleName};

const vec3 LABEL_PALETTE[${LABEL_PALETTE_SIZE}] = vec3[${LABEL_PALETTE_SIZE}](
${paletteGlsl()}
);

vec3 label_rgb(uint id) {
  return LABEL_PALETTE[(id - 1u) % ${LABEL_PALETTE_SIZE}u];
}

vec4 label_color(float value) {
  // The texture holds exact integers; the half-voxel guards the float round-trip.
  uint id = uint(max(0., value) + 0.5);
  if (id == 0u) return vec4(0.);
  vec3 rgb = label_rgb(id);
  if (id == ${moduleName}.selectedLabel) {
    return vec4(mix(rgb, vec3(1.), 0.5), min(1., ${moduleName}.labelOpacity + 0.3));
  }
  return vec4(rgb, ${moduleName}.labelOpacity);
}
`

/**
 * Defining `fs:DECKGL_PROCESS_INTENSITY` is what suppresses viv's contrast ramp, so the
 * id reaches the colour hook unwindowed. The body has to be non-empty: viv detects the
 * hook by the truthiness of the injected string.
 */
export const PASS_LABEL_INTENSITY = `
  intensity = max(0., intensity);
`

/** What both extensions read off the layer they are attached to. */
export interface LabelHost {
  props: { selectedLabel?: number; labelOpacity?: number }
  getModels(): { shaderInputs: { setProps(props: Record<string, unknown>): void } }[]
}

export const labelUniforms = (props: LabelHost['props']) => ({
  selectedLabel: clampLabelId(props.selectedLabel ?? 0),
  labelOpacity: clampOpacity(props.labelOpacity ?? 1),
})

const labelPaletteModule = {
  name: LABEL_MODULE_NAME,
  uniformTypes: labelUniformTypes,
  fs: labelShaderFs(LABEL_MODULE_NAME),
  inject: {
    'fs:DECKGL_PROCESS_INTENSITY': PASS_LABEL_INTENSITY,
    'fs:DECKGL_MUTATE_COLOR': `
  rgba = label_color(intensity[0]);
`,
  },
}

export interface LabelPaletteExtensionProps {
  /** Highlighted id; 0 selects nothing. */
  selectedLabel: number
  /** Alpha for every non-zero id, 0–1. */
  labelOpacity: number
}

/**
 * Colours a single-channel label plane on the `XRLayer` path. It replaces
 * `ColorPaletteExtension` rather than sitting beside it — both define
 * `fs:DECKGL_MUTATE_COLOR`, and only one may.
 */
export class LabelPaletteExtension extends VivLayerExtension {
  static override extensionName = 'LabelPaletteExtension'
  static defaultProps = {
    selectedLabel: { type: 'number', value: 0, compare: true },
    labelOpacity: { type: 'number', value: 1, compare: true },
  }

  getVivShaderTemplates() {
    return { modules: [labelPaletteModule] }
  }

  override updateState(
    params: Parameters<VivLayerExtension['updateState']>[0],
    extension: this,
  ): void {
    super.updateState(params, extension)
    const layer = this as unknown as LabelHost
    const uniforms = labelUniforms(layer.props)
    for (const model of layer.getModels()) {
      model.shaderInputs.setProps({ [LABEL_MODULE_NAME]: uniforms })
    }
  }
}

export const labelPlaneExtensions = () => [new LabelPaletteExtension()]

export interface LabelPlaneArgs {
  id: string
  plane: PlaneBuffer
  /** Quad in world units, [left, bottom, right, top] — the image plane's bounds. */
  bounds: [number, number, number, number]
  /** Overlay opacity, applied to every non-zero id. */
  opacity: number
  /** Selected cell id, or 0. */
  selectedLabel?: number
}

export interface LabelPlaneProps {
  id: string
  channelData: { data: ChannelTexture[]; width: number; height: number }
  bounds: [number, number, number, number]
  contrastLimits: [number, number][]
  channelsVisible: boolean[]
  selections: Record<string, number>[]
  dtype: 'Uint8' | 'Uint16' | 'Uint32'
  interpolation: 'nearest'
  opacity: number
  pickable: false
  selectedLabel: number
  labelOpacity: number
}

/**
 * Props for the label plane drawn over an ortho slice. `pickable` is false so the image
 * plane below keeps receiving deck picks; label selection goes through `/pixel`.
 */
export function labelPlaneProps(args: LabelPlaneArgs): LabelPlaneProps {
  const [height, width] = args.plane.shape
  return {
    id: args.id,
    channelData: { data: splitPlaneChannels(args.plane), width, height },
    bounds: args.bounds,
    // Unused — the ramp is suppressed — but `XRLayer` pads this array on every update.
    contrastLimits: [[0, 1]],
    channelsVisible: [true],
    selections: [{ c: 0 }],
    dtype: vivDtype(args.plane.dtype),
    interpolation: 'nearest',
    opacity: 1,
    pickable: false,
    selectedLabel: clampLabelId(args.selectedLabel ?? 0),
    labelOpacity: clampOpacity(args.opacity),
  }
}
