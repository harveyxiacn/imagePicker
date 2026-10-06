import { QueryClient } from '@tanstack/react-query'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { EditStack, PatchOp } from '@/api/types'
import { useEdit } from '@/stores/edit'
import { useGen, type GenTask } from '@/stores/gen'
import { useToasts } from '@/stores/toasts'
import { emptyStack } from './edit'
import { useHistory } from './history'
import { beforeFor, finalizeGen, genOnDone, genOnTask, resolveAfter } from './gen'

const server = new Map<number, EditStack>()
const putCalls: { id: number; stack: EditStack }[] = []

vi.mock('@/api/client', async () => {
  const actual = await vi.importActual<typeof import('@/api/client')>('@/api/client')
  return {
    ...actual,
    api: {
      ...actual.api,
      edits: vi.fn(async (id: number) => ({ photo_id: id, stack: server.get(id) ?? { version: 1, ops: [] }, updated_at: 1 })),
      putEdits: vi.fn(async (id: number, stack: EditStack) => {
        putCalls.push({ id, stack })
        server.set(id, stack)
        return { photo_id: id, stack, updated_at: 2, thumb_version: 'v' }
      }),
    },
  }
})

const patch = (asset: string, extra: Partial<PatchOp> = {}): PatchOp => ({ type: 'patch', kind: 'best_take', asset, rect: [0, 0, 0.1, 0.1], enabled: true, ...extra })
const stackOf = (...ops: EditStack['ops']): EditStack => ({ version: 1, ops })

const task = (over: Partial<GenTask> = {}): GenTask => ({
  taskId: 't1',
  kind: 'inpaint',
  photoId: 7,
  label: 'Remove bystanders',
  before: emptyStack(),
  baseFaceIds: [],
  done: 0,
  total: 0,
  state: 'running',
  startedAt: 0,
  ...over,
})

describe('generative task results', () => {
  let qc: QueryClient
  beforeEach(() => {
    qc = new QueryClient()
    server.clear()
    putCalls.length = 0
    useHistory.getState().clear()
    useGen.setState({ tasks: {}, results: {}, autoBase: null })
    useToasts.setState({ toasts: [], tasks: {} })
    useEdit.setState({ photoId: null, loaded: false, stack: emptyStack(), committed: emptyStack() })
  })

  it('resolveAfter: server stack wins when the user did not edit meanwhile, else patches are merged into the live stack', () => {
    const before = emptyStack()
    const srv = stackOf(patch('a'))
    expect(resolveAfter(before, srv, null)).toBe(srv)
    expect(resolveAfter(before, srv, emptyStack())).toBe(srv)
    const live = stackOf({ type: 'global', exposure: 0.4 })
    const merged = resolveAfter(before, srv, live)
    expect(merged.ops.map((o) => o.type)).toEqual(['patch', 'global'])
  })

  it('beforeFor: auto best take uses the snapshot of the photo it finally edited', () => {
    const snap = stackOf({ type: 'global', exposure: 1 })
    const t = task({ photoId: 7, before: emptyStack(), snapshots: { 7: emptyStack(), 9: snap } })
    expect(beforeFor(t, 7)).toBe(t.before)
    expect(beforeFor(t, 9)).toBe(snap)
    expect(beforeFor(t, 11)).toEqual(emptyStack())
  })

  it('finishing a task records exactly ONE undo step with before/after stacks (several patches at once)', async () => {
    server.set(7, stackOf(patch('a', { person_id: 1 }), patch('b', { person_id: 2 }), patch('c', { person_id: 3 })))
    useGen.getState().begin(task({ kind: 'besttake', label: 'Best expression for everyone' }))
    await finalizeGen(qc, 't1', true, null)
    const undoStack = useHistory.getState().undoStack
    expect(undoStack).toHaveLength(1)
    expect(undoStack[0].label).toBe('Best expression for everyone')
    expect(undoStack[0].edits).toHaveLength(1)
    expect(undoStack[0].edits![0].before).toEqual(emptyStack())
    expect(undoStack[0].edits![0].after.ops).toHaveLength(3)
    expect(useGen.getState().tasks.t1).toBeUndefined()
    expect(putCalls).toHaveLength(0) // the server already has it: nothing to write back
  })

  it('refreshes the open editor and keeps newer slider edits (patches come from the server)', async () => {
    server.set(7, stackOf(patch('a')))
    useEdit.setState({ photoId: 7, loaded: true, stack: stackOf({ type: 'global', exposure: 0.5 }), committed: emptyStack() })
    useGen.getState().begin(task())
    await finalizeGen(qc, 't1', true, null)
    const st = useEdit.getState()
    expect(st.stack.ops.map((o) => o.type).sort()).toEqual(['global', 'patch'])
    expect(st.committed).toEqual(st.stack)
    // the merged stack differs from the server's: it is written back
    expect(putCalls).toHaveLength(1)
    expect(putCalls[0].stack.ops.some((o) => o.type === 'global')).toBe(true)
    expect(useHistory.getState().undoStack).toHaveLength(1)
  })

  it('does not add a history entry when nothing changed, and reports failures as a toast', async () => {
    useGen.getState().begin(task())
    await finalizeGen(qc, 't1', false, 'out of memory')
    expect(useHistory.getState().undoStack).toHaveLength(0)
    expect(useToasts.getState().toasts.some((x) => x.kind === 'error' && x.text.includes('out of memory'))).toBe(true)
  })

  it('finalises only once when task.progress and *.done both arrive', async () => {
    server.set(7, stackOf(patch('a')))
    useGen.getState().begin(task())
    genOnDone(qc, { type: 'inpaint.done', photo_id: 7, ok: true, reason: null })
    genOnTask(qc, { type: 'task.progress', task_id: 't1', kind: 'inpaint', done: 4, total: 4, state: 'done' })
    await vi.waitFor(() => expect(useHistory.getState().undoStack).toHaveLength(1))
    await new Promise((r) => setTimeout(r, 20))
    expect(useHistory.getState().undoStack).toHaveLength(1)
  })

  it('besttake.done stores per-face results (warnings) for the editor', () => {
    genOnDone(qc, {
      type: 'besttake.done',
      photo_id: 7,
      results: [
        { base_face_id: 31, ok: true, warnings: ['large_pose_change'], reason: null },
        { base_face_id: 32, ok: false, warnings: [], reason: 'face_occluded' },
      ],
    })
    const r = useGen.getState().results
    expect(r[31].warnings).toEqual(['large_pose_change'])
    expect(r[32].ok).toBe(false)
    expect(useToasts.getState().toasts.some((x) => x.kind === 'error')).toBe(true)
  })

  it('progress events update the running task', () => {
    useGen.getState().begin(task())
    genOnTask(qc, { type: 'task.progress', task_id: 't1', kind: 'inpaint', done: 2, total: 4, state: 'running' })
    expect(useGen.getState().tasks.t1).toMatchObject({ done: 2, total: 4, state: 'running' })
  })

  it('auto best take: the finished base photo becomes the editor base', async () => {
    const snap = stackOf({ type: 'global', exposure: 1 })
    server.set(9, stackOf({ type: 'global', exposure: 1 }, patch('a', { person_id: 1 })))
    useGen.getState().begin(task({ kind: 'besttake', photoId: 7, snapshots: { 7: emptyStack(), 9: snap } }))
    genOnDone(qc, { type: 'besttake.done', photo_id: 9, results: [{ base_face_id: 91, ok: true, warnings: [], reason: null }] })
    await vi.waitFor(() => expect(useGen.getState().autoBase?.photoId).toBe(9))
    const e = useHistory.getState().undoStack[0].edits![0]
    expect(e.photoId).toBe(9)
    expect(e.before).toEqual(snap)
    expect(e.after.ops.some((o) => o.type === 'patch')).toBe(true)
  })
})
