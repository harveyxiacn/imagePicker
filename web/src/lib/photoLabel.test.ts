import i18next from 'i18next'
import { beforeAll, describe, expect, it } from 'vitest'
import { en } from '@/i18n/en'
import { zhCN } from '@/i18n/zh-CN'
import { makePhoto } from './fixtures'
import { photoLabel } from './photoLabel'

const i = i18next.createInstance()
beforeAll(async () => {
  await i.init({
    lng: 'zh-CN',
    resources: { 'zh-CN': { translation: zhCN }, en: { translation: en } },
    interpolation: { escapeValue: false },
  })
})
const tIn = (lng: string) => {
  const fixed = i.getFixedT(lng)
  return (k: string, o?: Record<string, unknown>) => String(fixed(k, o as never))
}

describe('photoLabel', () => {
  it('is just the file name for a plain photo', () => {
    const p = makePhoto(1, { file_name: 'a.jpg' })
    expect(photoLabel(p, tIn('zh-CN'))).toBe('a.jpg')
    expect(photoLabel(p, tIn('en'))).toBe('a.jpg')
  })

  it('lists every status in zh', () => {
    const p = makePhoto(2, {
      file_name: 'b.cr3',
      user_rating: 4,
      flag: 1,
      color_label: 'red',
      has_edits: true,
      issues: ['blurry', 'closed_eyes'],
    })
    expect(photoLabel(p, tIn('zh-CN'), { count: 3 })).toBe('b.cr3，4 星，精选，色标：红，已编辑，连拍堆栈，共 3 张，有人闭眼，模糊')
  })

  it('lists every status in en', () => {
    const p = makePhoto(3, { file_name: 'c.jpg', user_rating: 1, flag: -1, color_label: 'blue', missing: true })
    expect(photoLabel(p, tIn('en'))).toBe('c.jpg, 1 star, Reject, Label: Blue, File missing')
  })

  it('falls back to the AI suggestion only without a user rating', () => {
    const t = tIn('en')
    expect(photoLabel(makePhoto(4, { ai_rating: 3.46, user_rating: null }), t)).toContain('AI suggestion 3.5')
    expect(photoLabel(makePhoto(4, { ai_rating: 3.46, user_rating: 2 }), t)).not.toContain('AI')
  })

  it('names duplicates by kind and ignores a single-photo stack', () => {
    const t = tIn('zh-CN')
    expect(photoLabel(makePhoto(5, { duplicate_group_id: 9, duplicate_kind: 'exact' }), t)).toContain('完全相同')
    expect(photoLabel(makePhoto(5, { duplicate_group_id: 9, duplicate_kind: 'near' }), t)).toContain('重新保存')
    expect(photoLabel(makePhoto(6), t, { count: 1 })).not.toContain('堆栈')
  })
})
