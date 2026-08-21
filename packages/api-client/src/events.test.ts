import { describe, expect, it, vi } from 'vitest'
import { ApiClient, type FetchLike } from './client'
import { EventStream, type WebSocketLike } from './events'
import type { JobState, Versions } from './schemas'

const versions: Versions = { sessionId: 'sess-1', image: 3, labels: 0, graph: 1, settings: 0 }

const dims = { t: 2, c: 2, z: 3, y: 4, x: 5 }

const projectInfo = {
  sessionId: 'sess-1',
  sourcePath: '/data/sample.zarr',
  projectPath: '/data/sample.zarr.cellstudio',
  dims,
  dtype: 'u16',
  scale: null,
  levels: [{ index: 0, dims, chunks: dims, factor: [1, 1, 1] }],
  channels: [],
  versions,
  layout: { hostile: false, amplification: { xy: 1, xz: 1, yz: 1 }, affectedViews: [] },
  hasLabels: false,
}

const runningJob: JobState = {
  id: 'job-1',
  kind: 'proxy',
  progress: 0.4,
  status: 'running',
  message: null,
}

class FakeSocket implements WebSocketLike {
  onopen: ((ev: never) => void) | null = null
  onclose: ((ev: never) => void) | null = null
  onerror: ((ev: never) => void) | null = null
  onmessage: ((ev: MessageEvent<unknown>) => void) | null = null
  closed = false

  constructor(readonly url: string) {}

  close(): void {
    this.closed = true
  }

  open(): void {
    this.onopen?.(undefined as never)
  }

  drop(): void {
    this.onclose?.(undefined as never)
  }

  deliver(frame: unknown): void {
    this.onmessage?.(new MessageEvent('message', { data: JSON.stringify(frame) }))
  }

  deliverRaw(data: string): void {
    this.onmessage?.(new MessageEvent('message', { data }))
  }
}

function harness(opts: { jobs?: JobState[] } = {}) {
  const counts = { ticket: 0, project: 0, jobs: 0 }
  const fetch: FetchLike = async (url) => {
    const body = (payload: unknown) =>
      new Response(JSON.stringify(payload), {
        headers: { 'content-type': 'application/json', 'x-cellstudio-session': 'sess-1' },
      })
    if (url.endsWith('/ws-ticket')) {
      counts.ticket += 1
      return body({ ticket: `ticket-${counts.ticket}` })
    }
    if (url.endsWith('/project')) {
      counts.project += 1
      return body(projectInfo)
    }
    if (url.endsWith('/jobs')) {
      counts.jobs += 1
      return body(opts.jobs ?? [])
    }
    throw new Error(`unexpected request ${url}`)
  }

  const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'tok', fetch })
  const sockets: FakeSocket[] = []
  const errors: unknown[] = []
  const stream = new EventStream(api, {
    webSocket: (url) => {
      const socket = new FakeSocket(url)
      sockets.push(socket)
      return socket
    },
    minBackoffMs: 1,
    maxBackoffMs: 4,
    onError: (err) => errors.push(err),
  })
  return { api, stream, sockets, counts, errors }
}

function socketAt(sockets: FakeSocket[], index: number): FakeSocket {
  const socket = sockets[index]
  if (socket === undefined) throw new Error(`no socket at index ${index}`)
  return socket
}

describe('EventStream', () => {
  it('authenticates with a ws ticket in the URL query', async () => {
    const { stream, sockets, counts } = harness()
    stream.connect()

    await vi.waitFor(() => expect(sockets).toHaveLength(1))

    expect(socketAt(sockets, 0).url).toBe('ws://127.0.0.1:7777/events?ticket=ticket-1')
    expect(counts.ticket).toBe(1)
    stream.close()
  })

  it('returns a disposer from on() that stops delivery', async () => {
    const { stream, sockets } = harness()
    const seen: string[] = []
    const dispose = stream.on('job', (event) => seen.push(event.job.id))
    stream.connect()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))
    const socket = socketAt(sockets, 0)

    socket.deliver({ type: 'job', job: runningJob })
    expect(seen).toEqual(['job-1'])

    dispose()
    socket.deliver({ type: 'job', job: { ...runningJob, id: 'job-2' } })
    expect(seen).toEqual(['job-1'])
    stream.close()
  })

  it('re-fetches versions and job state on every reconnect', async () => {
    const { stream, sockets, counts } = harness({ jobs: [runningJob] })
    const seenVersions: Versions[] = []
    const seenJobs: string[] = []
    stream.on('versions', (event) => seenVersions.push(event.versions))
    stream.on('job', (event) => seenJobs.push(event.job.id))
    stream.connect()

    await vi.waitFor(() => expect(sockets).toHaveLength(1))
    socketAt(sockets, 0).open()
    await vi.waitFor(() => expect(seenVersions).toHaveLength(1))
    expect(counts.project).toBe(1)
    expect(counts.jobs).toBe(1)
    expect(seenJobs).toEqual(['job-1'])

    socketAt(sockets, 0).drop()
    await vi.waitFor(() => expect(sockets).toHaveLength(2))
    expect(socketAt(sockets, 1).url).toBe('ws://127.0.0.1:7777/events?ticket=ticket-2')

    socketAt(sockets, 1).open()
    await vi.waitFor(() => expect(seenVersions).toHaveLength(2))
    expect(counts.project).toBe(2)
    expect(counts.jobs).toBe(2)
    expect(seenVersions[1]).toEqual(versions)
    stream.close()
  })

  it('ignores unknown and malformed frames but keeps serving known ones', async () => {
    const { stream, sockets, errors } = harness()
    const seen: string[] = []
    stream.on('graphChanged', (event) => seen.push(String(event.graphVersion)))
    stream.connect()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))
    const socket = socketAt(sockets, 0)

    socket.deliver({ type: 'somethingNewer', payload: { whatever: true } })
    socket.deliver({ type: 'versions' })
    socket.deliverRaw('not json at all')
    expect(seen).toEqual([])

    socket.deliver({ type: 'graphChanged', graphVersion: 9, tracks: [1, 2] })
    expect(seen).toEqual(['9'])
    expect(errors).toEqual([])
    stream.close()
  })

  it('stops reconnecting after close()', async () => {
    const { stream, sockets } = harness()
    stream.connect()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))

    stream.close()
    socketAt(sockets, 0).drop()
    await new Promise((resolve) => setTimeout(resolve, 20))

    expect(sockets).toHaveLength(1)
    expect(socketAt(sockets, 0).closed).toBe(true)
  })

  it('retries when the ticket fetch fails and reports the failure', async () => {
    let fail = true
    const fetch: FetchLike = async (url) => {
      if (url.endsWith('/ws-ticket') && fail) return new Response('nope', { status: 503 })
      return new Response(JSON.stringify({ ticket: 'ticket-ok' }), {
        headers: { 'content-type': 'application/json' },
      })
    }
    const api = new ApiClient({ baseUrl: 'http://127.0.0.1:7777', token: 'tok', fetch })
    const sockets: FakeSocket[] = []
    const errors: unknown[] = []
    const stream = new EventStream(api, {
      webSocket: (url) => {
        const socket = new FakeSocket(url)
        sockets.push(socket)
        return socket
      },
      minBackoffMs: 1,
      maxBackoffMs: 4,
      onError: (err) => errors.push(err),
    })

    stream.connect()
    await vi.waitFor(() => expect(errors.length).toBeGreaterThan(0))
    fail = false
    await vi.waitFor(() => expect(sockets).toHaveLength(1))

    expect(socketAt(sockets, 0).url).toBe('ws://127.0.0.1:7777/events?ticket=ticket-ok')
    stream.close()
  })
})
