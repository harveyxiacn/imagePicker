import { useTranslation } from 'react-i18next'
import { helpGroups } from '@/lib/keymap'
import { useUi } from '@/stores/ui'
import { Modal } from './Modal'

const GROUPS = helpGroups()

export function HelpOverlay() {
  const { t } = useTranslation()
  const open = useUi((s) => s.helpOpen)
  const setOpen = useUi((s) => s.setHelpOpen)
  return (
    <Modal open={open} onOpenChange={setOpen} title={t('help.title')} width="max-w-3xl">
      <div className="grid gap-x-8 gap-y-5 md:grid-cols-2" data-testid="help-overlay">
        {GROUPS.map((g) => (
          <section key={g.group}>
            <h3 className="mb-2 text-xs font-semibold tracking-wide text-muted uppercase">{t(`keyGroup.${g.group}`)}</h3>
            <ul className="flex flex-col gap-1.5">
              {g.items.map((it) => (
                <li key={it.desc + it.scopes.join()} className="flex items-center justify-between gap-3">
                  <span>
                    {t(`keys.${it.desc}`)}
                    {it.scopes.length === 1 && <span className="ml-1.5 text-xs text-faint">({t(`scope.${it.scopes[0]}`)})</span>}
                  </span>
                  <span className="flex shrink-0 items-center gap-1.5">
                    {it.keys.map((combo, i) => (
                      <span key={i} className="flex items-center gap-0.5">
                        {i > 0 && <span className="px-0.5 text-faint">/</span>}
                        {combo.map((k, j) => (
                          <kbd key={j}>{k}</kbd>
                        ))}
                      </span>
                    ))}
                  </span>
                </li>
              ))}
            </ul>
          </section>
        ))}
        <section>
          <h3 className="mb-2 text-xs font-semibold tracking-wide text-muted uppercase">{t('help.touch')}</h3>
          <ul className="flex flex-col gap-1.5 text-muted">
            <li>{t('help.swipe')}</li>
            <li>{t('help.doubleTap')}</li>
            <li>{t('help.wheel')}</li>
          </ul>
        </section>
      </div>
    </Modal>
  )
}
