import { ColorPaletteExtension, XRLayer } from '@hms-dbmi/viv'
import type { Dtype, PlaneBuffer } from '@cellstudio/api-client'
import type { ChannelState } from '../state/nav'
import { GammaExtension, clampGamma } from './gamma'

/** viv's dtype names, which drive its sampler type and texture format. */
export const vivDtype = (dtype: Dtype): 'Uint8' | 'Uint16' | 'Uint32' =>
  dtype === 'u8' ? 'Uint8' : dtype === 'u16' ? 'Uint16' : 'Uint32'

const arrayFor = (dtype: Dtype, data: ArrayBuffer, offset: number, length: number) =>
  dtype === 'u8'
    ? new Uint8Array(data, offset, length)
    : dtype === 'u16'
      ? new Uint16Array(data, offset * 2, length)
      : new Uint32Array(data, offset * 4, length)

export type ChannelTexture = Uint8Array | Uint16Array | Uint32Array

/** Splits a `/slice` response into one view per packed channel; the planes are contiguous. */
export function splitPlaneChannels(plane: PlaneBuffer): ChannelTexture[] {
  const [height, width] = plane.shape
  const pixels = height * width
  const out: ChannelTexture[] = []
  for (let c = 0; c < plane.channels; c += 1) {
    out.push(arrayFor(plane.dtype, plane.data, c * pixels, pixels))
  }
  return out
}

export const hexToRgb = (color: string): [number, number, number] => {
  const hex = color.replace('#', '')
  const full =
    hex.length === 3
      ? hex
          .split('')
          .map((c) => c + c)
          .join('')
      : hex
  const n = Number.parseInt(full.slice(0, 6) || '0', 16)
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255]
}

export interface OrthoPlaneArgs {
  id: string
  plane: PlaneBuffer
  /** Quad in world units, [left, bottom, right, top] — y down, as viv's image layers use. */
  bounds: [number, number, number, number]
  /** Visible channels in the order the server packed them. */
  channels: ChannelState[]
  /** Selection per packed channel, so viv sizes the shader to the real channel count. */
  selections: Record<string, number>[]
  opacity?: number
}

export interface OrthoPlaneProps {
  id: string
  channelData: { data: ChannelTexture[]; width: number; height: number }
  bounds: [number, number, number, number]
  contrastLimits: [number, number][]
  channelsVisible: boolean[]
  colors: [number, number, number][]
  gammas: number[]
  selections: Record<string, number>[]
  dtype: 'Uint8' | 'Uint16' | 'Uint32'
  interpolation: 'nearest'
  opacity: number
  pickable: true
}

/**
 * Props for one assembled ortho plane. Windowing, color and gamma are all GPU-side, so
 * changing any of them re-renders without touching the plane cache.
 */
export function orthoPlaneProps(args: OrthoPlaneArgs): OrthoPlaneProps {
  const [height, width] = args.plane.shape
  return {
    id: args.id,
    channelData: { data: splitPlaneChannels(args.plane), width, height },
    bounds: args.bounds,
    contrastLimits: args.channels.map((c) => [c.window[0], c.window[1]]),
    channelsVisible: args.channels.map((c) => c.visible),
    colors: args.channels.map((c) => hexToRgb(c.color)),
    gammas: args.channels.map((c) => clampGamma(c.gamma)),
    selections: args.selections,
    dtype: vivDtype(args.plane.dtype),
    interpolation: 'nearest',
    opacity: args.opacity ?? 1,
    pickable: true,
  }
}

/**
 * One quad, no tile pyramid — the same viv XRLayer fragment path the multiscale XY layer
 * uses, so windowing and LUTs are identical across orientations.
 */
export class OrthoPlaneLayer extends XRLayer {
  static override layerName = 'OrthoPlaneLayer'
}

export const orthoPlaneExtensions = () => [new ColorPaletteExtension(), new GammaExtension()]
