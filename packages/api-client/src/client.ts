// Wire contract with cellstudio-server. The server implements exactly these names.
//
// Auth        every request: `Authorization: Bearer <token>`; the WS ticket is fetched over HTTP.
// Headers     x-cellstudio-session  session id — on every response once a project is open
//             x-cellstudio-shape    packed row-major extents, comma separated:
//                                     /slice  -> "c,h,w"      /volume -> "z,y,x"
//             x-cellstudio-dtype    "u8" | "u16" | "u32"
//             x-cellstudio-level    pyramid level the bytes were read from
// Binary      /slice and /volume bodies are little-endian application/octet-stream, tightly
//             packed (no padding between channels or planes), length == product(shape) * itemsize.
// JSON        camelCase field names exactly as typed in schemas.ts (serde: rename_all = "camelCase").
//
// Request/response shapes this client assumes beyond the design's endpoint sketch:
//   POST /project/open      {"path": string}                    -> ProjectInfo
//   GET  /project           404 when no project is open         -> ProjectInfo
//   GET  /slice             ?layer&axis&t&cs=1,2&pos&level      -> binary plane
//   GET  /volume            ?layer&t&c&level                    -> binary volume
//   GET  /pixel             ?layer&t&c&z&y&x                    -> {"value": number}
//   GET  /histogram         ?layer&t&c                          -> Histogram
//   GET  /cells             ?t0&t1&bbox=y0,y1,x0,x1             -> CellRow[]
//   GET  /lineage           ?cell                               -> LineageTree
//   GET  /jobs                                                  -> JobState[]
//   POST /import/{kind}     {"path": string}                    -> {"id": string}
//   GET  /settings                                              -> opaque JSON object
//   PUT  /settings          opaque JSON object                  -> 2xx, body ignored
//   GET  /ws-ticket                                             -> {"ticket": string}

import { z } from 'zod'
import {
  CellRow,
  Dtype,
  Histogram,
  JobState,
  LineageTree,
  ProjectInfo,
  type Bbox,
  type LayerId,
  type PlaneBuffer,
  type VolumeBuffer,
} from './schemas'

const SESSION_HEADER = 'x-cellstudio-session'
const SHAPE_HEADER = 'x-cellstudio-shape'
const DTYPE_HEADER = 'x-cellstudio-dtype'
const LEVEL_HEADER = 'x-cellstudio-level'

const DEFAULT_MAX_BUFFER_BYTES = 256 * 1024 * 1024

const ITEM_SIZE: Record<Dtype, number> = { u8: 1, u16: 2, u32: 4 }

export type FetchLike = (url: string, init?: RequestInit) => Promise<Response>

export type Settings = Record<string, unknown>
export type JobId = string

export interface SliceQuery {
  layer: LayerId
  axis: 'xz' | 'yz'
  t: number
  cs: number[]
  pos: number
  level: number
}

export interface VolumeQuery {
  layer: LayerId
  t: number
  c: number
  level: number
}

export interface PixelQuery {
  layer: LayerId
  t: number
  c: number
  z: number
  y: number
  x: number
}

export interface HistogramQuery {
  layer: LayerId
  t: number
  c: number
}

export interface CellsQuery {
  t0: number
  t1: number
  bbox?: Bbox
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly url: string,
    readonly detail: string,
  ) {
    super(`HTTP ${status} from ${url}${detail ? `: ${detail}` : ''}`)
    this.name = 'ApiError'
  }
}

export class SchemaError extends Error {
  constructor(
    readonly url: string,
    readonly detail: string,
  ) {
    super(`malformed response from ${url}: ${detail}`)
    this.name = 'SchemaError'
  }
}

/** A response issued under a session the client no longer speaks for, so drop it. */
export class StaleSessionError extends Error {
  constructor(
    readonly expected: string,
    readonly received: string,
    readonly url: string,
  ) {
    super(`stale session from ${url}: expected ${expected}, got ${received}`)
    this.name = 'StaleSessionError'
  }
}

export class BufferTooLargeError extends Error {
  constructor(
    readonly bytes: number,
    readonly limit: number,
    readonly url: string,
  ) {
    super(`${url} declares ${bytes} bytes, over the ${limit} byte limit`)
    this.name = 'BufferTooLargeError'
  }
}

export function isStaleSession(err: unknown): err is StaleSessionError {
  return err instanceof StaleSessionError
}

export interface ApiClientConfig {
  baseUrl: string
  token: string
  /** Ceiling on one binary response; a larger declared payload is refused before allocating. */
  maxBufferBytes?: number
  fetch?: FetchLike
}

interface RequestOptions {
  method?: 'GET' | 'POST' | 'PUT'
  query?: Record<string, string | number | undefined>
  body?: unknown
  signal?: AbortSignal
  /** Take the response's session id as the client's own instead of comparing against it. */
  adoptSession?: boolean
}

/** One instance per backend session; every cache downstream of it dies with it. */
export class ApiClient {
  readonly baseUrl: string
  readonly maxBufferBytes: number
  readonly #token: string
  readonly #fetch: FetchLike | undefined
  #sessionId: string | null = null

  constructor(cfg: ApiClientConfig) {
    this.baseUrl = cfg.baseUrl.replace(/\/+$/, '')
    this.maxBufferBytes = cfg.maxBufferBytes ?? DEFAULT_MAX_BUFFER_BYTES
    this.#token = cfg.token
    this.#fetch = cfg.fetch
  }

  /** Session id last seen on a response; null until the first session-scoped reply. */
  get sessionId(): string | null {
    return this.#sessionId
  }

  async openProject(path: string): Promise<ProjectInfo> {
    const info = await this.#json(ProjectInfo, '/project/open', {
      method: 'POST',
      body: { path },
      adoptSession: true,
    })
    this.#sessionId = info.sessionId
    return info
  }

  async getProject(): Promise<ProjectInfo | null> {
    const { res, url } = await this.#send('/project')
    if (res.status === 404) {
      await drain(res)
      return null
    }
    return this.#parse(ProjectInfo, res, url)
  }

  async slice(q: SliceQuery, signal?: AbortSignal): Promise<PlaneBuffer> {
    const raw = await this.#binary(
      '/slice',
      { layer: q.layer, axis: q.axis, t: q.t, cs: q.cs.join(','), pos: q.pos, level: q.level },
      signal,
    )
    const [channels, height, width] = raw.shape
    return { shape: [height, width], channels, dtype: raw.dtype, level: raw.level, data: raw.data }
  }

  async volume(q: VolumeQuery, signal?: AbortSignal): Promise<VolumeBuffer> {
    const raw = await this.#binary(
      '/volume',
      { layer: q.layer, t: q.t, c: q.c, level: q.level },
      signal,
    )
    return { shape: raw.shape, dtype: raw.dtype, level: raw.level, data: raw.data }
  }

  async pixel(q: PixelQuery): Promise<number> {
    const { value } = await this.#json(PixelValue, '/pixel', { query: { ...q } })
    return value
  }

  histogram(q: HistogramQuery): Promise<Histogram> {
    return this.#json(Histogram, '/histogram', { query: { ...q } })
  }

  cellsWindow(q: CellsQuery): Promise<CellRow[]> {
    const bbox = q.bbox ? `${q.bbox.y0},${q.bbox.y1},${q.bbox.x0},${q.bbox.x1}` : undefined
    return this.#json(z.array(CellRow), '/cells', { query: { t0: q.t0, t1: q.t1, bbox } })
  }

  lineage(cellId: number): Promise<LineageTree> {
    return this.#json(LineageTree, '/lineage', { query: { cell: cellId } })
  }

  jobs(): Promise<JobState[]> {
    return this.#json(z.array(JobState), '/jobs')
  }

  async startImport(kind: 'tracks' | 'labels', path: string): Promise<JobId> {
    const { id } = await this.#json(JobRef, `/import/${kind}`, { method: 'POST', body: { path } })
    return id
  }

  getSettings(): Promise<Settings> {
    return this.#json(SettingsSchema, '/settings')
  }

  async putSettings(s: Settings): Promise<void> {
    const { res, url } = await this.#send('/settings', { method: 'PUT', body: s })
    await this.#ok(res, url)
    await drain(res)
  }

  /** One-time ticket for the event WebSocket, which cannot carry an Authorization header. */
  async wsTicket(): Promise<string> {
    const { ticket } = await this.#json(WsTicket, '/ws-ticket')
    return ticket
  }

  #url(path: string, query?: RequestOptions['query']): string {
    const url = new URL(this.baseUrl + path)
    for (const [key, value] of Object.entries(query ?? {})) {
      if (value !== undefined) url.searchParams.set(key, String(value))
    }
    return url.toString()
  }

  async #send(path: string, opts: RequestOptions = {}): Promise<{ res: Response; url: string }> {
    const url = this.#url(path, opts.query)
    const headers = new Headers({ authorization: `Bearer ${this.#token}` })
    let body: string | undefined
    if (opts.body !== undefined) {
      headers.set('content-type', 'application/json')
      body = JSON.stringify(opts.body)
    }
    const init: RequestInit = { method: opts.method ?? 'GET', headers, body, signal: opts.signal }
    const res = this.#fetch ? await this.#fetch(url, init) : await globalThis.fetch(url, init)

    const session = res.headers.get(SESSION_HEADER)
    if (session !== null) {
      if (opts.adoptSession || this.#sessionId === null) {
        this.#sessionId = session
      } else if (session !== this.#sessionId) {
        await drain(res)
        throw new StaleSessionError(this.#sessionId, session, url)
      }
    }
    return { res, url }
  }

  async #ok(res: Response, url: string): Promise<void> {
    if (res.ok) return
    const detail = await res.text().catch(() => '')
    throw new ApiError(res.status, url, detail.slice(0, 500))
  }

  async #json<T>(schema: z.ZodType<T>, path: string, opts?: RequestOptions): Promise<T> {
    const { res, url } = await this.#send(path, opts)
    return this.#parse(schema, res, url)
  }

  async #parse<T>(schema: z.ZodType<T>, res: Response, url: string): Promise<T> {
    await this.#ok(res, url)
    const text = await res.text()
    let raw: unknown
    try {
      raw = JSON.parse(text)
    } catch {
      throw new SchemaError(url, `body is not JSON (${text.slice(0, 200)})`)
    }
    const parsed = schema.safeParse(raw)
    if (!parsed.success) throw new SchemaError(url, z.prettifyError(parsed.error))
    return parsed.data
  }

  async #binary(
    path: string,
    query: RequestOptions['query'],
    signal?: AbortSignal,
  ): Promise<{ shape: [number, number, number]; dtype: Dtype; level: number; data: ArrayBuffer }> {
    const { res, url } = await this.#send(path, { query, signal })
    await this.#ok(res, url)

    const shape = readShape(res, url)
    const dtype = readDtype(res, url)
    const level = readLevel(res, url)

    const expected = shape[0] * shape[1] * shape[2] * ITEM_SIZE[dtype]
    const declared = Number(res.headers.get('content-length') ?? expected)
    const claim = Math.max(expected, Number.isFinite(declared) ? declared : expected)
    if (claim > this.maxBufferBytes) {
      await drain(res)
      throw new BufferTooLargeError(claim, this.maxBufferBytes, url)
    }

    const data = await res.arrayBuffer()
    if (data.byteLength !== expected) {
      throw new SchemaError(url, `body is ${data.byteLength} bytes, headers imply ${expected}`)
    }
    return { shape, dtype, level, data }
  }
}

const PixelValue = z.object({ value: z.number() })
const JobRef = z.object({ id: z.string() })
const WsTicket = z.object({ ticket: z.string() })
const SettingsSchema = z.record(z.string(), z.unknown())

function readShape(res: Response, url: string): [number, number, number] {
  const header = res.headers.get(SHAPE_HEADER)
  const parts = header?.split(',').map((v) => Number(v.trim())) ?? []
  const [a, b, c] = parts
  if (
    parts.length !== 3 ||
    a === undefined ||
    b === undefined ||
    c === undefined ||
    ![a, b, c].every((v) => Number.isSafeInteger(v) && v >= 0)
  ) {
    throw new SchemaError(
      url,
      `${SHAPE_HEADER} must be three non-negative integers, got "${header}"`,
    )
  }
  return [a, b, c]
}

function readDtype(res: Response, url: string): Dtype {
  const parsed = Dtype.safeParse(res.headers.get(DTYPE_HEADER))
  if (!parsed.success) {
    throw new SchemaError(url, `${DTYPE_HEADER} is "${res.headers.get(DTYPE_HEADER)}"`)
  }
  return parsed.data
}

function readLevel(res: Response, url: string): number {
  const level = Number(res.headers.get(LEVEL_HEADER))
  if (!Number.isSafeInteger(level) || level < 0) {
    throw new SchemaError(url, `${LEVEL_HEADER} is "${res.headers.get(LEVEL_HEADER)}"`)
  }
  return level
}

// Release the socket without buffering a body we are about to discard.
async function drain(res: Response): Promise<void> {
  try {
    await res.body?.cancel()
  } catch {
    /* already consumed or bodyless */
  }
}
