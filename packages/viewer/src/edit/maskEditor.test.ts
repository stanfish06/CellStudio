import { describe, expect, it } from 'vitest'
import type { EditResult, PlaneBuffer } from '@cellstudio/api-client'
import { MaskEditor, type LabelPlaneView } from './maskEditor'
import { FakeApi, makeLabelPlane } from '../test/data'

const settle = () => new Promise((r) => setTimeout(r, 0))

const DIMS = [3, 64, 64] as const

const setup = (auto = true) => {
  const api = new FakeApi()
  api.auto = auto
  const commits: EditResult[] = []
  const errors: unknown[] = []
  const labels: number[] = []
  const editor = new MaskEditor({
    api,
    onCommit: (r) => commits.push(r),
    onError: (e) => errors.push(e),
    onLabel: (l) => labels.push(l),
  })
  editor.configure({ dims: [DIMS[0], DIMS[1], DIMS[2]], scale: null, storePresent: false })
  return { api, editor, commits, errors, labels }
}

const view = (o: Partial<LabelPlaneView> = {}): LabelPlaneView => ({
  axis: 'z',
  index: 1,
  t: 0,
  factor: [1, 1, 1],
  shape: [DIMS[1], DIMS[2]],
  level: 0,
  version: -1,
  ...o,
})

const at = (plane: PlaneBuffer | null, y: number, x: number): number => {
  if (!plane) return 0
  const [, width] = plane.shape
  return new Uint32Array(plane.data)[y * width + x] ?? 0
}

const filled = (value: number): PlaneBuffer => {
  const plane = makeLabelPlane(DIMS[1], DIMS[2])
  new Uint32Array(plane.data).fill(value)
  return plane
}

const stroke = (
  editor: MaskEditor,
  o: {
    label?: number | null
    tool?: 'brush' | 'eraser'
    centre?: [number, number, number]
    radius?: number
  } = {},
) =>
  editor.begin({
    t: 0,
    tool: o.tool ?? 'brush',
    radius: o.radius ?? 3,
    plane: { axis: 'z', index: 1 },
    centre: o.centre ?? [1, 32.5, 32.5],
    selection: o.label === undefined ? 5 : o.label,
  })

describe('MaskEditor stroke', () => {
  it('echoes the first stamp in the same tick, with no request', () => {
    const { api, editor } = setup()
    expect(stroke(editor)).toBe(true)
    expect(at(editor.planeBuffer(view(), null), 32, 32)).toBe(5)
    expect(api.strokeCalls).toHaveLength(0)
    expect(editor.pendingWrites).toBe(0)
  })

  it('coalesces stamps at r/3 and posts once on release', async () => {
    const { api, editor } = setup()
    stroke(editor, { radius: 9 })
    // A tenth of a radius is the same stamp; five ninths is the next one.
    editor.move([1, 32.5, 33.5])
    editor.move([1, 32.5, 37.5])
    editor.end()
    await settle()
    expect(api.strokeCalls).toHaveLength(1)
    const body = api.strokeCalls[0]
    expect(body?.stamps).toEqual([
      [1.5, 32.5, 32.5],
      [1.5, 32.5, 37.5],
    ])
    expect(body).toMatchObject({ t: 0, label: 5, mode: 'paint', radius: 9, plane: 'z', only: null })
  })

  it('takes a leased id when nothing is selected, and selects it', async () => {
    const { api, editor, labels } = setup()
    editor.ensureLease()
    await settle()
    expect(stroke(editor, { label: null })).toBe(true)
    expect(labels).toEqual([1000])
    editor.end()
    await settle()
    expect(api.strokeCalls[0]?.label).toBe(1000)
  })

  it('refuses a new-id stroke with no lease rather than echoing one the server rejects', () => {
    const { api, editor, errors } = setup()
    api.auto = false
    expect(stroke(editor, { label: null })).toBe(false)
    expect(editor.pendingOps).toBe(0)
    expect(editor.planeBuffer(view(), null)).toBe(null)
    expect(errors).toHaveLength(1)
  })

  it('scopes the eraser to the selection, and clears anything without one', () => {
    const { editor } = setup()
    const base = filled(9)
    new Uint32Array(base.data)[32 * DIMS[2] + 32] = 7
    stroke(editor, { tool: 'eraser', label: 7 })
    const scoped = editor.planeBuffer(view(), base)
    expect(at(scoped, 32, 32)).toBe(0)
    expect(at(scoped, 32, 33)).toBe(9)
    editor.cancel()
    stroke(editor, { tool: 'eraser', label: null })
    const unscoped = editor.planeBuffer(view(), base)
    expect(at(unscoped, 32, 32)).toBe(0)
    expect(at(unscoped, 32, 33)).toBe(0)
  })

  it('discards a cancelled stroke and writes nothing', async () => {
    const { api, editor } = setup()
    stroke(editor)
    editor.move([1, 32.5, 40.5])
    editor.cancel()
    await settle()
    expect(editor.pendingOps).toBe(0)
    expect(editor.planeBuffer(view(), null)).toBe(null)
    expect(api.strokeCalls).toHaveLength(0)
  })

  it('reports the store once an edit created one', async () => {
    const { editor } = setup()
    expect(editor.labelsPresent).toBe(false)
    stroke(editor)
    editor.end()
    await settle()
    expect(editor.labelsPresent).toBe(true)
  })
})

describe('MaskEditor pending log', () => {
  it('keeps a queued stroke visible when the one in flight commits', async () => {
    const { api, editor } = setup(false)
    stroke(editor, { label: 5 })
    editor.end()
    stroke(editor, { label: 6, centre: [1, 40.5, 40.5] })
    editor.end()
    await settle()
    // One write in flight, the second queued behind it.
    expect(api.openEdits).toBe(1)
    expect(editor.pendingWrites).toBe(2)

    api.settleEditAt(0, { version: 2 })
    await settle()
    // The refetched base contains A, so only A leaves the log; B still draws over it.
    const base = filled(0)
    new Uint32Array(base.data)[32 * DIMS[2] + 32] = 5
    const drawn = editor.planeBuffer(view({ version: 2 }), base)
    expect(at(drawn, 32, 32)).toBe(5)
    expect(at(drawn, 40, 40)).toBe(6)
    expect(editor.pendingOps).toBe(1)
    expect(api.openEdits).toBe(2)
  })

  it('removes only the failed operation, leaving the queued one drawn', async () => {
    const { api, editor, errors } = setup(false)
    stroke(editor, { label: 5 })
    editor.end()
    stroke(editor, { label: 6, centre: [1, 40.5, 40.5] })
    editor.end()
    await settle()
    api.failEditAt(0)
    await settle()
    const drawn = editor.planeBuffer(view(), null)
    expect(at(drawn, 32, 32)).toBe(0)
    expect(at(drawn, 40, 40)).toBe(6)
    expect(editor.pendingOps).toBe(1)
    expect(errors).toHaveLength(1)
  })

  it('replays two overlapping operations in order, eraser included', () => {
    const { editor } = setup()
    stroke(editor, { label: 5 })
    editor.end()
    stroke(editor, { tool: 'eraser', label: 5, centre: [1, 34.5, 32.5] })
    const drawn = editor.planeBuffer(view(), null)
    // The paint is under the eraser's stamp at 34, and outside it at 30.
    expect(at(drawn, 34, 32)).toBe(0)
    expect(at(drawn, 30, 32)).toBe(5)
  })

  it('never patches the base: a level change re-derives from the level-0 operation', () => {
    const { editor } = setup()
    const base = filled(0)
    stroke(editor, { label: 5 })
    expect(at(editor.planeBuffer(view(), base), 32, 32)).toBe(5)
    const coarse = editor.planeBuffer(
      view({ factor: [1, 2, 2], shape: [DIMS[1] / 2, DIMS[2] / 2], level: 1 }),
      null,
    )
    expect(at(coarse, 16, 16)).toBe(5)
    // The authoritative buffer is untouched, so the next derive starts from it again.
    expect(at(base, 32, 32)).toBe(0)
    expect(at(editor.planeBuffer(view(), base), 32, 32)).toBe(5)
  })

  it('drops an operation only once a base at its version is being drawn', async () => {
    const { api, editor } = setup(false)
    stroke(editor, { label: 5 })
    editor.end()
    await settle()
    api.settleEditAt(0, { version: 4 })
    await settle()
    // Still drawn over the stale base the refetch is chasing.
    expect(at(editor.planeBuffer(view({ version: 3 }), null), 32, 32)).toBe(5)
    expect(editor.pendingOps).toBe(1)
    expect(editor.planeBuffer(view({ version: 4 }), null)).toBe(null)
    expect(editor.pendingOps).toBe(0)
  })

  it('applies the volume echo at the level the 3D view drew', () => {
    const { editor } = setup()
    editor.begin({
      t: 0,
      tool: 'brush',
      radius: 4,
      plane: null,
      centre: [1.5, 32.5, 32.5],
      selection: 5,
    })
    const volume = editor.volumeBuffer(
      { t: 0, factor: [1, 2, 2], dims: [DIMS[0], DIMS[1] / 2, DIMS[2] / 2], level: 1, version: -1 },
      null,
    )
    expect(volume?.shape).toEqual([3, 32, 32])
    const data = new Uint32Array(volume?.data ?? new ArrayBuffer(0))
    expect(data[(1 * 32 + 16) * 32 + 16]).toBe(5)
  })
})

describe('MaskEditor orb strokes', () => {
  it('refuses a new-label stroke with no lease rather than echoing what the server rejects', () => {
    const { editor, errors } = setup()
    const started = editor.begin({
      t: 0,
      tool: 'brush',
      radius: 8,
      plane: null,
      centre: [1.5, 32.5, 32.5],
      selection: null,
    })
    expect(started).toBe(false)
    expect(errors).toHaveLength(1)
  })

  it('posts a plane-less stroke once a lease is in hand', async () => {
    const { api, editor } = setup()
    editor.ensureLease()
    await settle()
    expect(
      editor.begin({
        t: 0,
        tool: 'brush',
        radius: 8,
        plane: null,
        centre: [1.5, 32.5, 32.5],
        selection: null,
      }),
    ).toBe(true)
    editor.move([1.5, 34, 34])
    editor.end()
    await settle()
    expect(api.strokeCalls).toHaveLength(1)
    expect(api.strokeCalls[0]?.plane).toBe(null)
  })
})

describe('MaskEditor writes', () => {
  it('queues delete, undo and redo behind the strokes, one in flight', async () => {
    const { api, editor } = setup(false)
    stroke(editor)
    editor.end()
    editor.deleteMask(0, 5)
    editor.undo()
    editor.redo()
    await settle()
    expect(editor.pendingWrites).toBe(4)
    expect(api.openEdits).toBe(1)
    expect(api.deleteCalls).toHaveLength(0)
    for (let i = 0; i < 4; i++) {
      api.settleEditAt(i, { version: 2 + i })
      await settle()
    }
    expect(api.deleteCalls).toEqual([{ t: 0, label: 5 }])
    expect(api.editCalls).toEqual(['undo', 'redo'])
    expect(editor.pendingWrites).toBe(0)
  })

  it('routes every committed result to the version path', async () => {
    const { editor, commits } = setup()
    stroke(editor)
    editor.end()
    editor.deleteMask(0, 5)
    await settle()
    expect(commits.map((c) => c.sessionId)).toEqual(['session-1', 'session-1'])
    const deletion = commits[1]
    expect(deletion?.domain === 'graph' ? [] : deletion?.removed).toEqual([5])
  })
})
