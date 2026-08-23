import { randomBytes } from 'node:crypto'
import { spawn, type ChildProcess } from 'node:child_process'
import { existsSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { app } from 'electron'
import type { BackendInfo, BackendState } from '../shared/bridge'

const HEALTH_TIMEOUT_MS = 3000
const HEALTH_POLL_MS = 50
const SHUTDOWN_GRACE_MS = 5000
const MAX_RESPAWNS = 3
const PORT_LINE_TIMEOUT_MS = 5000

export interface SupervisorEvents {
  onState(state: BackendState, generation: number): void
}

function resolveBinary(): string {
  const packaged = join(process.resourcesPath ?? '', 'bin', 'cellstudio-server')
  if (app.isPackaged) return packaged
  const root = join(app.getAppPath(), '..', '..')
  const debug = join(root, 'target', 'debug', 'cellstudio-server')
  const release = join(root, 'target', 'release', 'cellstudio-server')
  // newest wins: a stale release build must not shadow the one the dev loop just made
  const built = [release, debug].filter(existsSync)
  built.sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)
  return built[0] ?? debug
}

export class BackendSupervisor {
  #child: ChildProcess | null = null
  #info: BackendInfo | null = null
  #generation = 0
  #respawns = 0
  #stopping = false
  #state: BackendState = 'down'

  constructor(private readonly events: SupervisorEvents) {}

  get info(): BackendInfo | null {
    return this.#info
  }

  get state(): BackendState {
    return this.#state
  }

  async start(): Promise<BackendInfo | null> {
    this.#stopping = false
    this.#setState('starting')

    const binary = resolveBinary()
    if (!existsSync(binary)) {
      this.#setState('fatal')
      return null
    }

    const token = randomBytes(32).toString('hex')
    const child = spawn(binary, ['--token', token], {
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, RUST_LOG: process.env.RUST_LOG ?? 'info' },
    })
    this.#child = child
    child.stderr?.on('data', (d: Buffer) => process.stderr.write(`[backend] ${d}`))
    child.once('exit', (code, signal) => this.#onExit(code, signal))

    let port: number
    try {
      port = await readPortLine(child)
    } catch {
      this.#setState('fatal')
      return null
    }

    const baseUrl = `http://127.0.0.1:${port}`
    if (!(await waitForHealth(baseUrl, token))) {
      this.#setState('down')
      child.kill('SIGKILL')
      return null
    }

    this.#generation += 1
    this.#info = { baseUrl, token, generation: this.#generation }
    this.#respawns = 0
    this.#setState('ready')
    return this.#info
  }

  async stop(): Promise<void> {
    this.#stopping = true
    const child = this.#child
    this.#child = null
    this.#info = null
    if (!child || child.exitCode !== null) return

    child.kill('SIGTERM')
    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => {
        child.kill('SIGKILL')
        resolve()
      }, SHUTDOWN_GRACE_MS)
      child.once('exit', () => {
        clearTimeout(timer)
        resolve()
      })
    })
    this.#setState('down')
  }

  #onExit(code: number | null, signal: NodeJS.Signals | null): void {
    if (this.#stopping) return
    this.#info = null
    process.stderr.write(`[backend] exited unexpectedly (code=${code} signal=${signal})\n`)

    if (this.#respawns >= MAX_RESPAWNS) {
      this.#setState('fatal')
      return
    }
    this.#respawns += 1
    this.#setState('down')
    const backoff = 250 * 2 ** (this.#respawns - 1)
    setTimeout(() => {
      void this.start()
    }, backoff)
  }

  #setState(state: BackendState): void {
    this.#state = state
    this.events.onState(state, this.#generation)
  }
}

function readPortLine(child: ChildProcess): Promise<number> {
  return new Promise((resolve, reject) => {
    let buffer = ''
    const timer = setTimeout(() => reject(new Error('no port line')), PORT_LINE_TIMEOUT_MS)
    child.stdout?.on('data', (chunk: Buffer) => {
      buffer += chunk.toString()
      for (const line of buffer.split('\n')) {
        const trimmed = line.trim()
        if (!trimmed.startsWith('{')) continue
        try {
          const parsed: unknown = JSON.parse(trimmed)
          const port = (parsed as { port?: unknown }).port
          if (typeof port === 'number') {
            clearTimeout(timer)
            resolve(port)
            return
          }
        } catch {}
      }
    })
  })
}

async function waitForHealth(baseUrl: string, token: string): Promise<boolean> {
  const deadline = Date.now() + HEALTH_TIMEOUT_MS
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`${baseUrl}/health`, {
        headers: { authorization: `Bearer ${token}` },
      })
      if (res.ok) return true
    } catch {}
    await new Promise((r) => setTimeout(r, HEALTH_POLL_MS))
  }
  return false
}
