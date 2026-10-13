import { useTranslation } from 'react-i18next'
import { FONT_SCALE_DEFAULT, FONT_SCALE_MAX, FONT_SCALE_MIN, FONT_SCALE_STEP } from '@/lib/appearance'
import { helpGroups } from '@/lib/keymap'
import { useUi, type Lang, type Theme } from '@/stores/ui'
import { Card, Row, Select, Toggle } from './controls'
import { useSetting } from './useSetting'

const GROUPS = helpGroups()

/** 快捷键: read-only table generated from the keymap registry. */
export function ShortcutsSection() {
  const { t } = useTranslation()
  return (
    <div className="flex flex-col gap-4" data-testid="settings-keys">
      <p className="text-xs text-muted">{t('settings.keys.hint')}</p>
      {GROUPS.map((g) => (
        <Card key={g.group} title={t(`keyGroup.${g.group}`)}>
          <table className="w-full">
            <tbody>
              {g.items.map((it) => (
                <tr key={it.desc + it.scopes.join()} className="border-b border-line last:border-0">
                  <td className="px-4 py-1.5">
                    {t(`keys.${it.desc}`)}
                    {it.scopes.length === 1 && <span className="ml-1.5 text-xs text-faint">({t(`scope.${it.scopes[0]}`)})</span>}
                  </td>
                  <td className="px-4 py-1.5 text-right">
                    <span className="inline-flex flex-wrap items-center justify-end gap-1.5">
                      {it.keys.map((combo, i) => (
                        <span key={i} className="flex items-center gap-0.5">
                          {i > 0 && <span className="px-0.5 text-faint">/</span>}
                          {combo.map((k, j) => (
                            <kbd key={j}>{k}</kbd>
                          ))}
                        </span>
                      ))}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
      ))}
    </div>
  )
}

/** 语言与外观: language + theme (local preference, mirrored to the server settings). */
export function AppearanceSection() {
  const { t } = useTranslation()
  const { set } = useSetting()
  const lang = useUi((s) => s.lang)
  const theme = useUi((s) => s.theme)
  const contrast = useUi((s) => s.contrast)
  const fontScale = useUi((s) => s.fontScale)
  return (
    <Card title={t('settings.appearance.title')} testId="settings-appearance">
      <Row label={t('settings.appearance.language')}>
        <Select<Lang>
          label={t('settings.appearance.language')}
          testId="setting-language"
          value={lang}
          options={[
            { value: 'zh-CN', label: '简体中文' },
            { value: 'en', label: 'English' },
          ]}
          onChange={(v) => {
            useUi.getState().setLang(v)
            set('language', v)
          }}
        />
      </Row>
      <Row label={t('settings.appearance.theme')} hint={t('settings.appearance.themeHint')}>
        <Select<Theme>
          label={t('settings.appearance.theme')}
          testId="setting-theme"
          value={theme}
          options={(['dark', 'light', 'system'] as const).map((v) => ({ value: v, label: t(`app.theme_${v}`) }))}
          onChange={(v) => {
            useUi.getState().setTheme(v)
            set('theme', v)
          }}
        />
      </Row>
      <Row label={t('settings.appearance.highContrast')} hint={t('settings.appearance.highContrastHint')}>
        <Toggle
          label={t('settings.appearance.highContrast')}
          testId="setting-contrast"
          checked={contrast === 'high'}
          onChange={(v) => useUi.getState().setContrast(v ? 'high' : 'normal')}
        />
      </Row>
      <Row label={t('settings.appearance.fontScale')} hint={t('settings.appearance.fontScaleHint')}>
        <div className="flex items-center gap-2">
          <input
            type="range"
            min={FONT_SCALE_MIN}
            max={FONT_SCALE_MAX}
            step={FONT_SCALE_STEP}
            value={fontScale}
            aria-label={t('settings.appearance.fontScale')}
            aria-valuetext={`${fontScale}%`}
            className="w-36"
            onChange={(e) => useUi.getState().setFontScale(Number(e.target.value))}
            data-testid="setting-font-scale"
          />
          <span className="tnum w-10 text-right">{fontScale}%</span>
          <button className="btn" disabled={fontScale === FONT_SCALE_DEFAULT} onClick={() => useUi.getState().setFontScale(FONT_SCALE_DEFAULT)}>
            {t('settings.appearance.fontScaleReset')}
          </button>
        </div>
      </Row>
    </Card>
  )
}
