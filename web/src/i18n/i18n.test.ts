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
      ['issue', ['closed_eyes', 'blurry', 'overexposed', 'underexposed', 'noisy', 'tilted', 'bystander', 'bad_expression']],
      ['scene_type', ['portrait', 'group', 'landscape', 'food', 'architecture', 'night', 'pet', 'other']],
      ['reason', ['closed_eyes', 'blurry', 'overexposed', 'underexposed', 'noisy', 'sharp', 'great_expression', 'high_aesthetic', 'best_in_burst', 'bystander', 'bad_expression_talking', 'bad_expression_half_blink']],
      ['score', ['sharpness', 'exposure', 'noise', 'iqa', 'aesthetic', 'face', 'composition']],
      ['person', ['state_eyes_open', 'state_smiling', 'state_looking', 'state_subject', 'state_short_eyes_open', 'state_short_smiling', 'state_short_looking', 'state_short_subject', 'count_any', 'count_single', 'count_few', 'count_many', 'count_none', 'tri_off', 'tri_include', 'tri_exclude']],
      ['worker', ['state_stopped', 'state_starting', 'state_ready', 'state_busy', 'state_crashed', 'state_unavailable']],
      ['analysis', ['stage_analyzing', 'stage_grouping', 'stage_scoring', 'stage_clustering', 'profile_fast', 'profile_standard', 'profile_fast_desc', 'profile_standard_desc']],
      ['filter', ['sort_ai', 'sort_rating', 'issues_all', 'issues_none', 'issues_any', 'sharp_sharp', 'sharp_blurry', 'sharp_unknown']],
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
      ['settings', ['auto'].map((k) => `hardware.tier_${k}`)],
      ['models', ['commercial-ok', 'research-only', 'non-commercial', 'unknown'].map((k) => `usage_${k}`)],
      ['models', ['research-only', 'non-commercial', 'unknown'].flatMap((k) => [`hint_${k}`, `warn_${k}`])],
      ['models', ['permissive', 'research-only', 'non-commercial', 'undisclosed', 'separate-terms', 'not-recorded'].map((k) => `terms_${k}`)],
      ['settings', ['thumbs', 'previews', 'masks', 'edits', 'gen'].flatMap((k) => [`cache.kind_${k}`, `cache.kind_${k}_hint`])],
      ['settings', ['off', 'sidecar', 'modify_originals'].flatMap((k) => [`xmp.mode_${k}`, `xmp.mode_${k}_desc`])],
      ['settings', ['catalog'].map((k) => `nav.${k}`)],
      ['catalog', ['auto', 'replaced', 'corrupt'].map((k) => `kind_${k}`)],
      ['catalog', ['checking', 'ok', 'damaged', 'recovery'].map((k) => `state_${k}`)],
      ['faceConsent', ['purpose', 'local', 'off', 'wipe'].map((k) => `point_${k}`)],
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
      ['tasks', ['besttake', 'inpaint', 'enhance', 'hair', 'export', 'analysis'].map((k) => `kind_${k}`)],
      ['repair', ['best_take', 'hair', 'inpaint', 'denoise', 'face_restore', 'deshine', 'flyaway'].map((k) => `kind_${k}`)],
      ['hair', ['fuller', 'less_shine', 'tidier', 'sharper', 'similar_pose', 'earlier', 'same_session'].map((k) => `cue_${k}`)],
      ['hair', ['pose_too_different', 'donor_too_small', 'hair_not_found', 'donor_no_hair', 'face_not_found', 'face_too_small', 'face_not_aligned', 'not_composable', 'unknown'].map((k) => `reason_${k}`)],
      ['hair', ['pose_differs', 'large_fill', 'donor_small', 'seam', 'lighting_unmatched'].map((k) => `warn_${k}`)],
      ['hair', ['earlier', 'later', 'sameTime']],
      ['gen', ['hair_person_mismatch', 'hair_low_similarity', 'hair_pose_too_different', 'hair_donor_too_small']],
      ['tasks', ['running', 'cancelling', 'done', 'failed', 'cancelled', 'interrupted'].map((k) => `status_${k}`)],
      ['tasks', ['denoise', 'face_restore', 'deshine', 'flyaway'].map((k) => `op_${k}`)],
      ['hair', ['deshine', 'flyaway', 'deshineRun', 'flyawayRun']],
      ['history', ['deshine', 'flyaway']],
      ['tasks', ['photo', 'photoOp', 'export', 'session'].map((k) => `detail_${k}`)],
      ['settings', ['tasks'].map((k) => `nav.${k}`)],
    ]
    for (const [ns, keys] of families) for (const k of keys) expect(zh.has(`${ns}.${k}`), `${ns}.${k}`).toBe(true)
  })
})
