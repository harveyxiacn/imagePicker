import { describe, expect, it } from 'vitest'
import { reasonKey } from './reasons'

const has = (k: string) => ['besttake.reason_camera_moved', 'besttake.reason_unknown', 'hair.warn_seam'].includes(k)

describe('reasonKey', () => {
  it('uses the specific key when present', () => {
    expect(reasonKey('besttake', 'camera_moved', 'reason', has)).toBe('besttake.reason_camera_moved')
    expect(reasonKey('hair', 'seam', 'warn', has)).toBe('hair.warn_seam')
  })
  it('falls back to unknown for unseen or empty codes', () => {
    expect(reasonKey('besttake', 'brand_new_code', 'reason', has)).toBe('besttake.reason_unknown')
    expect(reasonKey('besttake', null, 'reason', has)).toBe('besttake.reason_unknown')
    expect(reasonKey('besttake', 'reason_unknown', 'reason', has)).toBe('besttake.reason_unknown')
  })
  it('every backend besttake code is translated in both dictionaries', async () => {
    const { en } = await import('@/i18n/en')
    const { zhCN } = await import('@/i18n/zh-CN')
    for (const c of ['camera_moved', 'large_pose_change', 'face_not_aligned', 'face_occluded', 'face_too_small', 'face_not_found', 'not_composable', 'decode_failed', 'error']) {
      expect((en.besttake as Record<string, string>)[`reason_${c}`], c).toBeTruthy()
      expect((zhCN.besttake as Record<string, string>)[`reason_${c}`], c).toBeTruthy()
    }
  })
})
