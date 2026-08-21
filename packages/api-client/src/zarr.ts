import { ApiError, type FetchLike } from './client'

// Structural mirror of zarrita's Readable + RangeReadable so this package needs no zarr dependency
// (zarrita lives in packages/viewer). A store built here is accepted wherever a FetchStore is.
export type AbsolutePath = `/${string}`
export type RangeQuery = { offset: number; length: number } | { suffixLength: number }

export interface ZarrStore {
  get(key: AbsolutePath, opts?: RequestInit): Promise<Uint8Array | undefined>
  getRange(
    key: AbsolutePath,
    range: RangeQuery,
    opts?: RequestInit,
  ): Promise<Uint8Array | undefined>
}

export interface StoreOptions {
  /** Layer version, sent as `?v=` for cache correctness; a bump means a new store. */
  version?: number
  fetch?: FetchLike
}

/** Store over the raw `/store` passthrough: stored bytes, decoded by zarrita in the renderer. */
export function makeStore(baseUrl: string, token: string, opts: StoreOptions = {}): ZarrStore {
  const root = `${baseUrl.replace(/\/+$/, '')}/store`

  const request = async (key: AbsolutePath, init: RequestInit): Promise<Uint8Array | undefined> => {
    const url = new URL(root + key)
    if (opts.version !== undefined) url.searchParams.set('v', String(opts.version))
    const headers = new Headers(init.headers)
    headers.set('authorization', `Bearer ${token}`)
    const target = url.toString()
    const withAuth: RequestInit = { ...init, headers }
    const res = opts.fetch
      ? await opts.fetch(target, withAuth)
      : await globalThis.fetch(target, withAuth)

    // An absent chunk is data, not a failure: zarrita substitutes the fill value.
    if (res.status === 404) return undefined
    if (!res.ok) {
      throw new ApiError(res.status, target, (await res.text().catch(() => '')).slice(0, 500))
    }
    return new Uint8Array(await res.arrayBuffer())
  }

  return {
    get: (key, init) => request(key, init ?? {}),
    getRange: (key, range, init) => {
      const headers = new Headers(init?.headers)
      headers.set('range', rangeHeader(range))
      return request(key, { ...init, headers })
    },
  }
}

function rangeHeader(range: RangeQuery): string {
  return 'suffixLength' in range
    ? `bytes=-${range.suffixLength}`
    : `bytes=${range.offset}-${range.offset + range.length - 1}`
}
