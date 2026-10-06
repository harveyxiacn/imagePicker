import { describe, expect, it } from 'vitest'
import { en } from './en'
import { zhCN } from './zh-CN'

function flatten(o: Record<string, unknown>, prefix = ''): string[] {
  return Object.entries(o).flatMap(([k, v]) =>
    v && typeof v === 'object' ? flatten(v as Record<string, unknown>, `${prefix}${k}.`) : [`${prefix}${k}`],
  )
}

// Raw source of every component / page / lib (excluding mocks, tests and the dictionaries themselves).
const files = import.meta.glob(['../components/**/*.tsx', '../pages/**/*.{ts,tsx}', '../lib/**/*.ts', '!../lib/**/*.test.ts'], {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>

describe('i18n', () => {
  const zh = new Set(flatten(zhCN))
  const enKeys = new Set(flatten(en))

  it('zh-CN and en define the same keys', () => {
    expect([...zh].filter((k) => !enKeys.has(k))).toEqual([])
    expect([...enKeys].filter((k) => !zh.has(k))).toEqual([])
  })

  it('every literal t("...") key used in the source exists', () => {
    const missing: string[] = []
    expect(Object.keys(files).length).toBeGreaterThan(20)
    for (const [file, text] of Object.entries(files)) {
      for (const m of text.matchAll(/\bt\(\s*'([A-Za-z0-9_.-]+)'/g)) {
        if (!zh.has(m[1])) missing.push(`${file}: ${m[1]}`)
      }
    }
    expect(missing).toEqual([])
  })

  it('dynamic key families are complete', () => {
    const families: [string, string[]][] = [
      ['issue', ['closed_eyes', 'blurry', 'overexposed', 'underexposed', 'noisy', 'tilted']],
      ['scene_type', ['portrait', 'group', 'landscape', 'food', 'architecture', 'night', 'pet', 'other']],
      ['reason', ['closed_eyes', 'blurry', 'overexposed', 'underexposed', 'noisy', 'sharp', 'great_expression', 'high_aesthetic', 'best_in_burst']],
      ['score', ['sharpness', 'exposure', 'noise', 'iqa', 'aesthetic', 'face', 'composition']],
      ['person', ['state_eyes_open', 'state_smiling', 'state_looking', 'state_subject', 'state_short_eyes_open', 'state_short_smiling', 'state_short_looking', 'state_short_subject', 'count_any', 'count_single', 'count_few', 'count_many', 'count_none', 'tri_off', 'tri_include', 'tri_exclude']],
      ['worker', ['state_stopped', 'state_starting', 'state_ready', 'state_busy', 'state_crashed', 'state_unavailable']],
      ['analysis', ['stage_analyzing', 'stage_grouping', 'stage_scoring', 'stage_clustering', 'profile_fast', 'profile_standard', 'profile_fast_desc', 'profile_standard_desc']],
      ['filter', ['sort_ai', 'sort_rating', 'issues_all', 'issues_none', 'issues_any']],
      ['keys', ['viewGroup', 'aiAccept', 'aiAcceptAll', 'stackToggle', 'stackToggleAll', 'groupPrev', 'groupNext', 'groupPick', 'groupPickNext', 'facesToggle', 'personFilter']],
      ['keyGroup', ['ai']],
      ['scope', ['group']],
      ['view', ['group', 'grid', 'loupe', 'compare']],
      ['adjust', ['exposure', 'contrast', 'highlights', 'shadows', 'whites', 'blacks', 'temp', 'tint', 'vibrance', 'saturation', 'clarity', 'dehaze']],
      ['lut', ['film_warm', 'film_cool', 'teal_orange', 'matte', 'bw']],
      ['preset', ['film_warm', 'film_cool', 'vivid', 'matte', 'bw_contrast', 'teal_orange', 'golden_hour', 'soft_portrait']],
      ['edit', ['auto_auto', 'auto_portrait', 'auto_landscape', 'save_idle', 'save_saving', 'save_error', 'hsl_h', 'hsl_s', 'hsl_l']],
      ['edit', ['red', 'orange', 'yellow', 'green', 'aqua', 'blue', 'purple', 'magenta'].map((b) => `band_${b}`)],
      ['edit', ['subject', 'sky', 'background', 'person', 'skin', 'hair', 'clothes'].map((k) => `target_${k}`)],
      ['edit', ['crop', 'global', 'local', 'lut', 'output_sharpen'].map((k) => `section_${k}`)],
      ['edit', ['shadows', 'midtones', 'highlights'].map((k) => `wheel_${k}`)],
      ['edit', ['radial', 'linear'].map((k) => `mask_${k}`)],
      ['edit', ['original', 'free'].map((k) => `aspect_${k}`)],
      ['keys', ['editOpen', 'editClose', 'beforeAfter', 'maskOverlay', 'cropMode', 'copySettings', 'pasteSettings']],
      ['scope', ['edit']],
      [
        'history',
        ['adjust', 'auto', 'resetBasic', 'curves', 'hsl', 'hslBand', 'grading', 'gradingWheel', 'gradingBalance', 'addLocal', 'removeLocal', 'invertMask', 'localAmount', 'localAdjust', 'moveMask', 'crop', 'straighten', 'resetCrop', 'sharpen', 'preset', 'lut', 'lutAmount', 'paste', 'sync', 'resetAll'],
      ],
      ['beauty', ['smooth', 'whiten', 'eye_brighten', 'teeth_whiten', 'dark_circles', 'slim', 'chin', 'eyes', 'nose', 'arms', 'legs', 'waist', 'lengthen_legs', 'level_natural', 'level_standard', 'level_refined']],
      ['collections', ['best_per_group', 'has_closed_eyes', 'undecided', 'edited'].map((k) => `builtin_${k}`)],
      ['taste', ['low_saturation', 'bright_exposure', 'warm_tone', 'smile_over_sharp', 'centered_subject', 'dislikes_blur'].map((k) => `trait_${k}`)],
      ['history', ['portrait', 'portraitLevel', 'applyProfile', 'portraitReset', 'applyProfiles']],
      ['people', ['view_cards', 'view_best']],
    ]
    for (const [ns, keys] of families) for (const k of keys) expect(zh.has(`${ns}.${k}`), `${ns}.${k}`).toBe(true)
  })
})
