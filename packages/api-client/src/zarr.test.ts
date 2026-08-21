import { describe, expect, it } from 'vitest'
import { ApiError, type FetchLike } from './client'
import { makeStore } from './zarr'

interface Call {
  url: string
  init: RequestInit | undefined
}

function recorder(handler: (url: string) => Response): { fetch: FetchLike; calls: Call[] } {
  const calls: Call[] = []
  return {
    calls,
    fetch: async (url, init) => {
      calls.push({ url, init })
      return handler(url)
    },
  }
}

function requireAt<T>(values: T[], index: number): T {
  const value = values[index]
  if (value === undefined) throw new Error(`no element at index ${index}`)
  return value
}

describe('makeStore', () => {
  it('injects the bearer header and addresses the raw /store passthrough', async () => {
    const { fetch, calls } = recorder(() => new Response(new Uint8Array([1, 2, 3])))
    const store = makeStore('http://127.0.0.1:7777/', 'tok', { version: 4, fetch })

    const bytes = await store.get('/0/3/1/0/0/0')

    expect(bytes).toEqual(new Uint8Array([1, 2, 3]))
    expect(requireAt(calls, 0).url).toBe('http://127.0.0.1:7777/store/0/3/1/0/0/0?v=4')
    expect(new Headers(requireAt(calls, 0).init?.headers).get('authorization')).toBe('Bearer tok')
  })

  it('maps 404 to undefined so the client supplies the fill value', async () => {
    const { fetch } = recorder(() => new Response('missing', { status: 404 }))
    const store = makeStore('http://127.0.0.1:7777', 'tok', { fetch })

    await expect(store.get('/0/9/9/9/9/9')).resolves.toBeUndefined()
    await expect(store.getRange('/0/9/9/9/9/9', { offset: 0, length: 8 })).resolves.toBeUndefined()
  })

  it('throws ApiError on any other failure status', async () => {
    const { fetch } = recorder(() => new Response('denied', { status: 401 }))
    const store = makeStore('http://127.0.0.1:7777', 'tok', { fetch })

    const err = await store.get('/.zgroup').catch((e: unknown) => e)

    expect(err).toBeInstanceOf(ApiError)
    expect((err as ApiError).status).toBe(401)
  })

  it('translates range queries into a Range header', async () => {
    const { fetch, calls } = recorder(() => new Response(new Uint8Array([9])))
    const store = makeStore('http://127.0.0.1:7777', 'tok', { fetch })

    await store.getRange('/0/0/0/0/0/0', { offset: 16, length: 32 })
    await store.getRange('/0/0/0/0/0/0', { suffixLength: 8 })

    expect(new Headers(requireAt(calls, 0).init?.headers).get('range')).toBe('bytes=16-47')
    expect(new Headers(requireAt(calls, 1).init?.headers).get('range')).toBe('bytes=-8')
  })

  it('leaves the version param off when no version is given', async () => {
    const { fetch, calls } = recorder(() => new Response(new Uint8Array()))
    const store = makeStore('http://127.0.0.1:7777', 'tok', { fetch })

    await store.get('/.zattrs')

    expect(requireAt(calls, 0).url).toBe('http://127.0.0.1:7777/store/.zattrs')
  })
})
