import { loadOmeZarrFromStore } from '@hms-dbmi/viv'

/** Minimum a zarrita store must offer; the api-client's `makeStore` supplies it. */
export interface ZarrStoreLike {
  get(key: string, opts?: RequestInit): Promise<Uint8Array | undefined>
}

/** viv's own pixel-source type, derived so `@vivjs/*` stays a transitive dependency. */
export type PixelSources = Awaited<ReturnType<typeof loadOmeZarrFromStore>>['data']
export type PixelSource = PixelSources[number]

export interface XyPyramid {
  /** Level 0 first, coarsest last — the order viv's multiscale layer expects. */
  levels: PixelSources
  tileSize: number
  dtype: string
  labels: string[]
  width: number
  height: number
}

/** Appends the layer version so a bump misses the browser HTTP cache. */
export function versionedStore(store: ZarrStoreLike, version: number): ZarrStoreLike {
  return {
    get: (key, opts) => store.get(`${key}${key.includes('?') ? '&' : '?'}v=${version}`, opts),
  }
}

/**
 * Builds viv pixel sources over the raw `/store` passthrough. zarrita decodes in the
 * renderer; the server never touches these bytes.
 */
export async function loadXyPyramid(store: ZarrStoreLike, version?: number): Promise<XyPyramid> {
  const source = version === undefined ? store : versionedStore(store, version)
  // viv bundles its own zarrita (0.5) but the store type is structural, so this crosses
  // the version boundary without a cast; viv only ever calls `get`.
  const { data: levels } = await loadOmeZarrFromStore(source)
  const base = levels[0]
  if (!base) throw new Error('store has no pyramid levels')
  const xIndex = base.labels.indexOf('x')
  const yIndex = base.labels.indexOf('y')
  return {
    levels,
    tileSize: base.tileSize,
    dtype: base.dtype,
    labels: base.labels,
    width: base.shape[xIndex] ?? 0,
    height: base.shape[yIndex] ?? 0,
  }
}

/** One selection per visible channel, in the label order the loader reports. */
export const xySelections = (
  channels: readonly number[],
  t: number,
  z: number,
): Record<string, number>[] => channels.map((c) => ({ t, c, z }))
