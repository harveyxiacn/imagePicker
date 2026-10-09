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
      ['besttake', ['group_best', 'group_best_issue', 'fewer_missing', 'fewer_replacements', 'fewer_below_best', 'less_work', 'manual'].map((k) => `why_${k}`)],
      ['beauty', ['smooth', 'whiten', 'eye_brighten', 'teeth_whiten', 'dark_circles', 'slim', 'chin', 'eyes', 'nose', 'arms', 'legs', 'waist', 'lengthen_legs', 'level_natural', 'level_standard', 'level_refined']],
      ['collections', ['best_per_group', 'has_closed_eyes', 'undecided', 'edited'].map((k) => `builtin_${k}`)],
      ['taste', ['low_saturation', 'bright_exposure', 'warm_tone', 'smile_over_sharp', 'centered_subject', 'dislikes_blur'].map((k) => `trait_${k}`)],
      ['history', ['portrait', 'portraitLevel', 'applyProfile', 'portraitReset', 'applyProfiles']],
      ['people', ['view_cards', 'view_best']],
      ['assistant', ['filter', 'set_rating', 'set_flag', 'accept_ai', 'group_keep_top', 'scene_keep_top', 'apply_preset', 'auto_adjust', 'apply_profiles', 'besttake_auto', 'remove_bystanders', 'export', 'describe', 'suggest_edits'].map((k) => `tool_${k}`)],
      ['assistant', ['best', 'tone', 'picked', 'bystanders'].flatMap((k) => [`chip_${k}`, `chip_${k}_prompt`])],
      ['settings', ['hardware', 'analysis', 'faces', 'cache', 'render', 'lan', 'xmp', 'keys', 'appearance'].map((k) => `nav.${k}`)],
      ['settings', ['auto', 'hf', 'hf-mirror', 'modelscope'].map((k) => `models.source_${k}`)],
      ['settings', ['thumbs', 'previews', 'masks', 'edits', 'gen'].flatMap((k) => [`cache.kind_${k}`, `cache.kind_${k}_hint`])],
      ['settings', ['off', 'sidecar', 'sidecar_and_embedded'].flatMap((k) => [`xmp.mode_${k}`, `xmp.mode_${k}_desc`])],
      ['settings', ['loose', 'normal', 'strict'].map((k) => `analysis.strictness_${k}`)],
      ['settings', ['auto', 'rules', 'llm'].map((k) => `assistant.engine_${k}`)],
      ['settings', ['auto', 'gpu', 'cpu'].map((k) => `render.backend_${k}`)],
      ['onboarding', ['analyze', 'stacks', 'rating'].flatMap((k) => [`coach_${k}_title`, `coach_${k}`])],
      ['login', ['rateLimited', 'rateLimitedWait', 'wrong', 'lanDisabled', 'failed'].map((k) => `error_${k}`)],
      ['errors', ['forbiddenPath', 'forbidden', 'forbiddenOrigin', 'lanDisabled']],
      ['export', ['original', 'wechat', 'xiaohongshu', 'instagram'].map((k) => `preset_${k}`)],
      ['keys', ['assistant']],
      ['analysis', ['profile_lite', 'profile_lite_desc', 'profile_standard_remote', 'profile_standard_remote_desc']],
      ['settings', ['remote', 'devices'].map((k) => `nav.${k}`)],
      ['mobile', ['library', 'cull', 'people', 'settings'].map((k) => `nav.${k}`)],
      ['remote', ['invalid_code', 'pair_expired', 'host_unreachable', 'rate_limited', 'bad_qr'].map((k) => `err_${k}`)],
      ['remote', ['required', 'invalid'].flatMap((k) => [`url_${k}`, `code_${k}`])],
      ['remote', ['connected', 'offline', 'unpaired'].map((k) => `state_${k}`)],
    ]
    for (const [ns, keys] of families) for (const k of keys) expect(zh.has(`${ns}.${k}`), `${ns}.${k}`).toBe(true)
  })
})
