import { describe, expect, it } from 'vitest'
import type { TaskRecord } from '@/api/types'
import { en } from '@/i18n/en'
import { isLive, TASK_KINDS, TASK_STATUSES, taskDetail, taskPercent } from './tasks'

const rec = (over: Partial<TaskRecord>): TaskRecord => ({
  id: 'inpaint-1',
  kind: 'inpaint',
  status: 'running',
  params: {},
  done: 0,
  total: 0,
  error: null,
  created_at: 1,
  updated_at: 1,
  finished_at: null,
  cancellable: true,
  ...over,
})

describe('task history helpers', () => {
  it('live states poll, finished ones do not', () => {
    expect(TASK_STATUSES.filter(isLive)).toEqual(['running', 'cancelling'])
  })

  it('percent: clamped, full when done, empty without a total', () => {
    expect(taskPercent(rec({ done: 1, total: 4 }))).toBe(25)
    expect(taskPercent(rec({ done: 9, total: 4 }))).toBe(100)
    expect(taskPercent(rec({ done: 0, total: 0 }))).toBe(0)
    expect(taskPercent(rec({ status: 'done', done: 0, total: 0 }))).toBe(100)
  })

  it('detail: what the task worked on, from its params', () => {
    expect(taskDetail(rec({ params: { photo_id: 12, faces: 1 } }))).toEqual({ key: 'tasks.detail_photo', opts: { id: 12 } })
    expect(taskDetail(rec({ kind: 'enhance', params: { photo_id: 3, op: 'face_restore' } }))).toEqual({
      key: 'tasks.detail_photoOp',
      opts: { id: 3, op: 'face_restore' },
    })
    expect(taskDetail(rec({ kind: 'export', params: { count: 5, dest: 'D:\\out' } }))).toEqual({ key: 'tasks.detail_export', opts: { count: 5, dest: 'D:\\out' } })
    expect(taskDetail(rec({ kind: 'analysis', params: { session_id: 2, count: 40 } }))).toEqual({ key: 'tasks.detail_session', opts: { id: 2, count: 40 } })
    expect(taskDetail(rec({ kind: 'besttake', params: null }))).toBeNull()
    expect(taskDetail(rec({ kind: 'model_download', params: { photo_id: 1 } }))).toBeNull()
  })

  it('every kind and status has a label', () => {
    const t = en.tasks as Record<string, string>
    for (const k of TASK_KINDS) expect(t[`kind_${k}`]).toBeTruthy()
    for (const s of TASK_STATUSES) expect(t[`status_${s}`]).toBeTruthy()
  })
})
