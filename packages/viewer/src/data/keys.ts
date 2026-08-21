import type { LayerId } from '@cellstudio/api-client'
import type { OrthoAxis } from './api'

export interface PlaneKey {
  layer: LayerId
  axis: OrthoAxis
  level: number
  t: number
  c: readonly number[]
  index: number
  version: number
}

export interface VolumeKey {
  layer: LayerId
  level: number
  t: number
  c: number
  version: number
}

export const planeKeyId = (k: PlaneKey): string =>
  `${k.layer}/${k.axis}/${k.level}/${k.t}/${[...k.c].join('.')}/${k.index}/v${k.version}`

export const volumeKeyId = (k: VolumeKey): string =>
  `${k.layer}/${k.level}/${k.t}/${k.c}/v${k.version}`

export const samePlaneKey = (a: PlaneKey, b: PlaneKey): boolean => planeKeyId(a) === planeKeyId(b)

export const sameVolumeKey = (a: VolumeKey, b: VolumeKey): boolean =>
  volumeKeyId(a) === volumeKeyId(b)

/** Matches every key of a layer at a version other than `version` — invalidation on bump. */
export const staleOf = (layer: LayerId, version: number) => (id: string) =>
  id.startsWith(`${layer}/`) && !id.endsWith(`/v${version}`)
