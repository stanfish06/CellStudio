import { ServerEvent } from './schemas'
import type { ApiClient } from './client'

type EventType = ServerEvent['type']
type EventOf<T extends EventType> = Extract<ServerEvent, { type: T }>
type Listener = (event: ServerEvent) => void

/** `never` params on the open/close/error slots keep a DOM WebSocket structurally assignable. */
export interface WebSocketLike {
  close(): void
  onopen: ((ev: never) => void) | null
  onclose: ((ev: never) => void) | null
  onerror: ((ev: never) => void) | null
  onmessage: ((ev: MessageEvent<unknown>) => void) | null
}

export type WebSocketFactory = (url: string) => WebSocketLike

export interface EventStreamOptions {
  webSocket?: WebSocketFactory
  minBackoffMs?: number
  maxBackoffMs?: number
  /** Ticket, socket and re-sync failures land here; the stream keeps retrying regardless. */
  onError?: (err: unknown) => void
}

const DEFAULT_MIN_BACKOFF_MS = 250
const DEFAULT_MAX_BACKOFF_MS = 10_000
const MAX_BACKOFF_EXPONENT = 10

export class EventStream {
  readonly #api: ApiClient
  readonly #newSocket: WebSocketFactory
  readonly #minBackoff: number
  readonly #maxBackoff: number
  readonly #onError: ((err: unknown) => void) | undefined
  readonly #listeners = new Map<EventType, Set<Listener>>()
  #ws: WebSocketLike | null = null
  #timer: ReturnType<typeof setTimeout> | null = null
  #attempt = 0
  #closed = true

  constructor(api: ApiClient, opts: EventStreamOptions = {}) {
    this.#api = api
    this.#newSocket = opts.webSocket ?? ((url) => new WebSocket(url))
    this.#minBackoff = opts.minBackoffMs ?? DEFAULT_MIN_BACKOFF_MS
    this.#maxBackoff = opts.maxBackoffMs ?? DEFAULT_MAX_BACKOFF_MS
    this.#onError = opts.onError
  }

  connect(): void {
    if (!this.#closed) return
    this.#closed = false
    this.#attempt = 0
    void this.#open()
  }

  close(): void {
    this.#closed = true
    if (this.#timer !== null) {
      clearTimeout(this.#timer)
      this.#timer = null
    }
    const ws = this.#ws
    this.#ws = null
    if (ws) {
      ws.onopen = null
      ws.onclose = null
      ws.onerror = null
      ws.onmessage = null
      ws.close()
    }
  }

  on<T extends EventType>(type: T, cb: (event: EventOf<T>) => void): () => void {
    let set = this.#listeners.get(type)
    if (!set) {
      set = new Set<Listener>()
      this.#listeners.set(type, set)
    }
    const listener = cb as Listener
    set.add(listener)
    return () => {
      set.delete(listener)
    }
  }

  async #open(): Promise<void> {
    try {
      const ticket = await this.#api.wsTicket()
      if (this.#closed) return
      const ws = this.#newSocket(wsUrl(this.#api.baseUrl, ticket))
      this.#ws = ws
      ws.onopen = () => {
        this.#attempt = 0
        void this.#resync()
      }
      ws.onmessage = (ev) => this.#dispatch(ev.data)
      ws.onerror = () => this.#onError?.(new Error('event socket error'))
      ws.onclose = () => {
        if (this.#ws !== ws) return
        this.#ws = null
        this.#scheduleReconnect()
      }
    } catch (err) {
      this.#onError?.(err)
      this.#scheduleReconnect()
    }
  }

  // Events are hints; state is queryable. Every (re)connect re-reads versions and job state
  // instead of assuming the frames that would have carried them arrived.
  async #resync(): Promise<void> {
    try {
      const [project, jobs] = await Promise.all([this.#api.getProject(), this.#api.jobs()])
      if (this.#closed) return
      if (project) this.#emit({ type: 'versions', versions: project.versions })
      for (const job of jobs) this.#emit({ type: 'job', job })
    } catch (err) {
      this.#onError?.(err)
    }
  }

  #scheduleReconnect(): void {
    if (this.#closed || this.#timer !== null) return
    const exponent = Math.min(this.#attempt, MAX_BACKOFF_EXPONENT)
    const backoff = Math.min(this.#maxBackoff, this.#minBackoff * 2 ** exponent)
    this.#attempt += 1
    this.#timer = setTimeout(
      () => {
        this.#timer = null
        void this.#open()
      },
      backoff + Math.random() * backoff * 0.25,
    )
  }

  // Frames that don't match the schema — including event types a newer server introduces —
  // are dropped rather than thrown.
  #dispatch(data: unknown): void {
    if (typeof data !== 'string') return
    let raw: unknown
    try {
      raw = JSON.parse(data)
    } catch {
      return
    }
    const parsed = ServerEvent.safeParse(raw)
    if (parsed.success) this.#emit(parsed.data)
  }

  #emit(event: ServerEvent): void {
    const set = this.#listeners.get(event.type)
    if (!set) return
    for (const listener of [...set]) listener(event)
  }
}

function wsUrl(baseUrl: string, ticket: string): string {
  const url = new URL(`${baseUrl.replace(/\/+$/, '')}/events`)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  url.searchParams.set('ticket', ticket)
  return url.toString()
}
