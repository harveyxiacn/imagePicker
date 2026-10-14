import { describe, expect, it } from 'vitest'
import type { ImportSource, TaskRecord } from '@/api/types'
import { en } from '@/i18n/en'
import { zhCN } from '@/i18n/zh-CN'
import { cardCaption, cardReportOf, cardSummary, expandPattern, formatBytes, PATTERN_PRESETS, previewPath } from './cardImport'

const rec = (over: Partial<TaskRecord> & { report?: Record<string, unknown> }): Pick<TaskRecord, 'status' | 'error' | 'params'> => ({
  status: over.status ?? 'done',
  error: over.error ?? null,
  params: over.report ? { report: over.report } : (over.params ?? null),
})

const day = new Date(2026, 9, 3)

describe('destination pattern', () => {
  it('expands the tokens like the server does', () => {
    expect(expandPattern('{yyyy}/{yyyy-mm-dd}', day)).toEqual(['2026', '2026-10-03'])
    expect(expandPattern('{yyyymmdd} trip/{mm}{dd}', day)).toEqual(['20261003 trip', '1003'])
    expect(expandPattern('', day)).toEqual([])
    expect(expandPattern('  ', day)).toEqual([])
  })

  it('previews the destination with the separator it already uses', () => {
    expect(previewPath('D:\\Photos', '{yyyy}/{yyyy-mm-dd}', day)).toBe('D:\\Photos\\2026\\2026-10-03')
    expect(previewPath('/home/me/Pictures/', '{yyyy-mm-dd}', day)).toBe('/home/me/Pictures/2026-10-03')
    expect(previewPath('D:\\', '', day)).toBe('D:\\')
  })

  it('every preset is labelled in both languages', () => {
    for (const p of PATTERN_PRESETS) {
      expect((en.card as Record<string, string>)[`preset_${p.id}`]).toBeTruthy()
      expect((zhCN.card as Record<string, string>)[`preset_${p.id}`]).toBeTruthy()
    }
  })
})

describe('card captions', () => {
  it('formats sizes', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(1536)).toBe('1.5 KB')
    expect(formatBytes(64e9)).toBe('60 GB')
    expect(formatBytes(null)).toBe('')
    expect(formatBytes(-1)).toBe('')
  })

  it('describes photos and free space; truncated estimates say so', () => {
    const s: ImportSource = { label: 'EOS', path: 'G:\\', kind: 'removable', total_bytes: 64e9, free_bytes: 32e9, photo_estimate: 120, estimate_truncated: false }
    expect(cardCaption(s)).toEqual([
      { key: 'card.photos', opts: { n: 120 } },
      { key: 'card.space', opts: { free: '30 GB', total: '60 GB' } },
    ])
    expect(cardCaption({ ...s, estimate_truncated: true, total_bytes: null })).toEqual([{ key: 'card.photosMore', opts: { n: 120 } }])
  })
})

describe('result summary', () => {
  it('a clean import', () => {
    const s = cardSummary(rec({ report: { found: 12, copied: 10, bytes: 2048, skipped_existing: 2, session_id: 7 } }))
    expect(s.tone).toBe('success')
    expect(s.headline).toEqual({ key: 'card.result_ok', opts: { n: 10 } })
    expect(s.lines.map((l) => l.key)).toEqual(['card.sum_copied', 'card.sum_skipped'])
    expect(s.sessionId).toBe(7)
  })

  it('failures and an aborted run are a warning that lists what is left', () => {
    const s = cardSummary(
      rec({
        error: 'stopped after 3 files',
        report: { copied: 3, failed_count: 2, failed: [{ path: 'a.jpg', error: 'bad sector' }, { nope: 1 }], not_attempted: 4, aborted: 'disk full', session_id: 1 },
      }),
    )
    expect(s.tone).toBe('warning')
    expect(s.lines.map((l) => l.key)).toEqual(['card.sum_copied', 'card.sum_failed', 'card.sum_left', 'card.sum_aborted'])
    expect(s.failed).toEqual([{ path: 'a.jpg', error: 'bad sector' }])
  })

  it('cancelled, failed, interrupted and nothing-new', () => {
    expect(cardSummary(rec({ status: 'cancelled', report: { copied: 4, cancelled: true, session_id: 2 } })).headline).toEqual({ key: 'card.result_cancelled', opts: { n: 4 } })
    expect(cardSummary(rec({ status: 'failed', error: 'boom', report: { failed_count: 1 } })).tone).toBe('error')
    expect(cardSummary(rec({ status: 'interrupted' })).headline.key).toBe('card.result_interrupted')
    const none = cardSummary(rec({ report: { found: 5, skipped_existing: 5 } }))
    expect(none.tone).toBe('muted')
    expect(none.headline.key).toBe('card.result_nothing')
    expect(none.sessionId).toBeNull()
    expect(cardSummary(rec({})).headline.key).toBe('card.result_nothing')
  })

  it('parses a report defensively', () => {
    expect(cardReportOf({ params: null })).toBeNull()
    expect(cardReportOf({ params: { report: 'x' } })).toBeNull()
    expect(cardReportOf({ params: { report: { copied: 'many', session_id: 'x' } } })).toMatchObject({ copied: 0, session_id: null, failed: [] })
  })

  it('every summary key exists in both languages', () => {
    const keys = [
      'result_ok', 'result_partial', 'result_cancelled', 'result_failed', 'result_nothing', 'result_interrupted',
      'sum_copied', 'sum_reused', 'sum_skipped', 'sum_failed', 'sum_left', 'sum_aborted',
    ]
    for (const k of keys) {
      expect((en.card as Record<string, string>)[k]).toBeTruthy()
      expect((zhCN.card as Record<string, string>)[k]).toBeTruthy()
    }
  })
})
