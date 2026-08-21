import type { Bbox, CellRow, LayerId, PlaneBuffer, VolumeBuffer } from '@cellstudio/api-client'

export type OrthoAxis = 'xz' | 'yz'

/**
 * The subset of `ApiClient` the viewer's pixel plane uses. 
 */
export interface PixelApi {
  slice(
    q: { layer: LayerId; axis: OrthoAxis; t: number; cs: number[]; pos: number; level: number },
    signal?: AbortSignal,
  ): Promise<PlaneBuffer>
  volume(
    q: { layer: LayerId; t: number; c: number; level: number },
    signal?: AbortSignal,
  ): Promise<VolumeBuffer>
  pixel(
    q: { layer: LayerId; t: number; c: number; z: number; y: number; x: number },
    signal?: AbortSignal,
  ): Promise<number>
  cellsWindow(q: { t0: number; t1: number; bbox?: Bbox }, signal?: AbortSignal): Promise<CellRow[]>
}
