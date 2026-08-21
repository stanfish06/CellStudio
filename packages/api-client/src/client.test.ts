import { describe, expect, it, vi } from 'vitest'
import {
  ApiClient,
  ApiError,
  BufferTooLargeError,
  SchemaError,
  StaleSessionError,
  isStaleSession,
  type FetchLike,
} from './client'

const SESSION = 'x-cellstudio-session'

const dims = { t: 2, c: 2, z: 3, y: 4, x: 5 }

const projectInfo = {
  sessionId: 'sess-1',
  sourcePath: '/data/sample.zarr',
  projectPath: '/data/sample.zarr.cellstudio',
  dims,
  dtype: 'u16',
  scale: { z: 2, y: 0.603, x: 0.603 },
  levels: [{ index: 0, dims, chunks: dims, factor: [1, 1, 1] }],
  channels: [{ name: 'C0', color: 'ff0000', window: [0, 4000] }],
  versions: { sessionId: 'sess-1', image: 1, labels: 0, graph: 0, settings: 0 },
  layout: { hostile: false, amplification: { xy: 1, xz: 3, yz: 3 }, affectedViews: [] },
  hasLabels: false,
}

interface Call {
  url: string
  init: RequestInit | undefined
}

function recorder(handler: (url: string, init?: RequestInit) => Response): {
  fetch: FetchLike
  calls: Call[]
} {
  const calls: Call[] = []
  return {
    calls,
    fetch: async (url, init) => {
      calls.push({ url, init })
      return handler(url, init)
    },
  }
}

function json(body: unknown, init: { status?: number; headers?: Record<string, string> } = {}) {
  return new Response(JSON.stringify(body), {
    status: init.status ?? 200,
    headers: { 'content-type': 'application/json', ...init.headers },
  })
}

function binary(bytes: ArrayBuffer, headers: Record<string, string>) {
  return new Response(bytes, { headers })
}

function requireAt<T>(values: T[], index: number): T {
  const value = values[index]
  if (value === undefined) throw new Error(`no element at index ${index}`)
  return value
}

function authOf(call: Call): string | null {
  return new Headers(call.init?.headers).get('authorization')
}

describe('ApiClient JSON path', () => {
  it('parses a well-formed ProjectInfo and adopts its session', async () => {
    const { fetch, calls } = recorder(() => json(projectInfo, { headers: { [SESSION]: 'sess-1' } }))
    const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777/', token: 'tok', fetch })

    const info = await api.openProject('/data/sample.zarr')

    expect(info.dims.x).toBe(5)
    expect(api.sessionId).toBe('sess-1')
    expect(requireAt(calls, 0).url).toBe('http://127.0.0.1:7777/project/open')
    expect(requireAt(calls, 0).init?.method).toBe('POST')
    expect(requireAt(calls, 0).init?.body).toBe('{"path":"/data/sample.zarr"}')
  })

  it('rejects a malformed ProjectInfo with the offending field in the message', async () => {
    const broken = { ...projectInfo, dims: { ...dims, x: 'wide' }, channels: 'nope' }
    const { fetch } = recorder(() => json(broken))
    const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'tok', fetch })

    const err = await api.openProject('/data/sample.zarr').catch((e: unknown) => e)

    expect(err).toBeInstanceOf(SchemaError)
    expect((err as SchemaError).message).toContain('dims.x')
    expect((err as SchemaError).message).toContain('channels')
  })

  it('sends the bearer token on every request, including through the global fetch', async () => {
    const { fetch, calls } = recorder(() => json([]))
    const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'secret-token', fetch })

    await api.jobs()
    await api.cellsWindow({ t0: 0, t1: 4, bbox: { y0: 1, y1: 2, x0: 3, x1: 4 } })

    expect(calls.map(authOf)).toEqual(['Bearer secret-token', 'Bearer secret-token'])
    expect(requireAt(calls, 1).url).toContain('bbox=1%2C2%2C3%2C4')

    const global = vi.fn<FetchLike>(async () => json([]))
    vi.stubGlobal('fetch', global)
    await new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'secret-token' }).jobs()
    vi.unstubAllGlobals()

    const init = global.mock.calls[0]?.[1] as RequestInit | undefined
    expect(new Headers(init?.headers).get('authorization')).toBe('Bearer secret-token')
  })

  it('maps a 404 on /project to null and a 500 to ApiError', async () => {
    const api = (status: number) =>
      new ApiClient({
        baseUrl: 'http://127.0.0.1:7777',
        token: 'tok',
        fetch: recorder(() => json({ error: 'boom' }, { status })).fetch,
      })

    await expect(api(404).getProject()).resolves.toBeNull()
    await expect(api(500).getProject()).rejects.toBeInstanceOf(ApiError)
  })

  it('reads pixel values and import job ids', async () => {
    const { fetch, calls } = recorder((url) =>
      url.includes('/pixel') ? json({ value: 1284 }) : json({ id: 'job-7' }),
    )
    const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'tok', fetch })

    expect(await api.pixel({ layer: 'image', t: 3, c: 1, z: 2, y: 10, x: 11 })).toBe(1284)
    expect(await api.startImport('tracks', '/data/tracks.json')).toBe('job-7')
    expect(requireAt(calls, 0).url).toContain('layer=image&t=3&c=1&z=2&y=10&x=11')
    expect(requireAt(calls, 1).url).toBe('http://127.0.0.1:7777/import/tracks')
  })
})

describe('ApiClient binary path', () => {
  const planeHeaders = {
    'x-cellstudio-shape': '2,3,4',
    'x-cellstudio-dtype': 'u16',
    'x-cellstudio-level': '2',
    [SESSION]: 'sess-1',
  }

  it('parses shape, dtype and level headers into a PlaneBuffer', async () => {
    const samples = new Uint16Array(2 * 3 * 4).map((_, i) => i)
    const { fetch, calls } = recorder(() => binary(samples.buffer, planeHeaders))
    const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'tok', fetch })

    const plane = await api.slice({
      layer: 'image',
      axis: 'xz',
      t: 5,
      cs: [0, 2],
      pos: 17,
      level: 2,
    })

    expect(plane.shape).toEqual([3, 4])
    expect(plane.channels).toBe(2)
    expect(plane.dtype).toBe('u16')
    expect(plane.level).toBe(2)
    expect(new Uint16Array(plane.data)[7]).toBe(7)
    expect(api.sessionId).toBe('sess-1')
    expect(requireAt(calls, 0).url).toContain('axis=xz&t=5&cs=0%2C2&pos=17&level=2')
  })

  it('parses a VolumeBuffer shape as z,y,x', async () => {
    const { fetch } = recorder(() =>
      binary(new Uint8Array(3 * 4 * 5).buffer, {
        'x-cellstudio-shape': '3,4,5',
        'x-cellstudio-dtype': 'u8',
        'x-cellstudio-level': '0',
      }),
    )
    const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'tok', fetch })

    const volume = await api.volume({ layer: 'image', t: 0, c: 1, level: 0 })

    expect(volume.shape).toEqual([3, 4, 5])
    expect(volume.data.byteLength).toBe(60)
  })

  // The body never completes, so a client that tried to buffer it would hang instead of throwing.
  it('refuses an over-size allocation without reading the body', async () => {
    let cancelled = false
    const { fetch } = recorder(
      () =>
        new Response(
          new ReadableStream<Uint8Array>({
            pull() {},
            cancel() {
              cancelled = true
            },
          }),
          {
            headers: {
              'x-cellstudio-shape': '4,4096,4096',
              'x-cellstudio-dtype': 'u16',
              'x-cellstudio-level': '0',
            },
          },
        ),
    )
    const api = new ApiClient({
      baseUrl: 'http://127.0.0.1:7777',
      token: 'tok',
      maxBufferBytes: 1024 * 1024,
      fetch,
    })

    const err = await api
      .slice({ layer: 'image', axis: 'yz', t: 0, cs: [0, 1, 2, 3], pos: 0, level: 0 })
      .catch((e: unknown) => e)

    expect(err).toBeInstanceOf(BufferTooLargeError)
    expect((err as BufferTooLargeError).bytes).toBe(4 * 4096 * 4096 * 2)
    expect((err as BufferTooLargeError).limit).toBe(1024 * 1024)
    expect(cancelled).toBe(true)
  })

  it('rejects a body whose length contradicts the headers', async () => {
    const { fetch } = recorder(() => binary(new Uint8Array(4).buffer, planeHeaders))
    const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'tok', fetch })

    await expect(
      api.slice({ layer: 'image', axis: 'xz', t: 0, cs: [0, 1], pos: 0, level: 2 }),
    ).rejects.toThrow(/body is 4 bytes, headers imply 48/)
  })

  it('rejects unusable binary headers', async () => {
    const { fetch } = recorder(() =>
      binary(new Uint8Array(4).buffer, {
        'x-cellstudio-shape': '3,4',
        'x-cellstudio-dtype': 'f32',
      }),
    )
    const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'tok', fetch })

    await expect(api.volume({ layer: 'image', t: 0, c: 0, level: 0 })).rejects.toBeInstanceOf(
      SchemaError,
    )
  })

  it('forwards an AbortSignal so a superseded request cancels', async () => {
    let forwarded: AbortSignal | null = null
    const api = new ApiClient({
      baseUrl: 'http://127.0.0.1:7777',
      token: 'tok',
      fetch: (_url, init) =>
        new Promise((_resolve, reject) => {
          forwarded = init?.signal ?? null
          init?.signal?.addEventListener('abort', () =>
            reject(new DOMException('aborted', 'AbortError')),
          )
        }),
    })

    const controller = new AbortController()
    const pending = api.slice(
      { layer: 'image', axis: 'xz', t: 0, cs: [0], pos: 0, level: 0 },
      controller.signal,
    )
    controller.abort()

    await expect(pending).rejects.toThrow(/aborted/)
    expect(forwarded).toBe(controller.signal)
  })
})

describe('ApiClient session identity', () => {
  it('throws StaleSessionError when a response carries a superseded session', async () => {
    let session = 'sess-1'
    const { fetch } = recorder(() => json(projectInfo, { headers: { [SESSION]: session } }))
    const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'tok', fetch })

    await api.openProject('/data/sample.zarr')
    session = 'sess-0'
    const err = await api.getProject().catch((e: unknown) => e)

    expect(isStaleSession(err)).toBe(true)
    expect((err as StaleSessionError).expected).toBe('sess-1')
    expect((err as StaleSessionError).received).toBe('sess-0')
  })

  it('re-pins the session on openProject and leaves it alone when the header is absent', async () => {
    let session: string | null = 'sess-1'
    const { fetch } = recorder(() =>
      session === null
        ? json([])
        : json(
            { ...projectInfo, sessionId: session, versions: { ...projectInfo.versions } },
            { headers: { [SESSION]: session } },
          ),
    )
    const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'tok', fetch })

    await api.openProject('/data/sample.zarr')
    session = 'sess-2'
    await api.openProject('/data/other.zarr')
    expect(api.sessionId).toBe('sess-2')

    session = null
    await expect(api.jobs()).resolves.toEqual([])
    expect(api.sessionId).toBe('sess-2')
  })
})
