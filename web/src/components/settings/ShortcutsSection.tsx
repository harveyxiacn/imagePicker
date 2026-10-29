import { Lock, RotateCcw } from 'lucide-react'
import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { usePatchSettings } from '@/api/queries'
import { eventToSpec, formatSpec, type Binding } from '@/lib/keymap'
import {
  actionLabel,
  actionRows,
  overridesPatch,
  planAssign,
  resetOne,
  sanitizeOverrides,
  type ActionRow,
  type Overrides,
} from '@/lib/shortcuts'
import { Card } from './controls'
import { useSetting } from './useSetting'

const GROUP_ORDER = ['rate', 'flag', 'nav', 'view', 'zoom', 'edit', 'ai', 'misc'] as const
const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta', 'AltGraph', 'CapsLock', 'OS', 'Hyper', 'Super'])

interface Pending {
  id: string
  chord: string
  with: Binding[]
  swap?: Overrides
}

function Chord({ spec }: { spec: string }) {
  return (
    <span className="flex items-center gap-0.5">
      {formatSpec(spec).map((k, j) => (
        <kbd key={j}>{k}</kbd>
      ))}
    </span>
  )
}

/**
 * 快捷键: editable table generated from the keymap registry. Click a binding, press the new chord; a chord that is
 * already taken in the same scope offers to swap the two actions. Overrides live in `settings.shortcuts`.
 */
export function ShortcutsSection() {
  const { t } = useTranslation()
  const { s } = useSetting()
  const patch = usePatchSettings()
  const overrides = useMemo(() => sanitizeOverrides(s?.shortcuts), [s?.shortcuts])
  const rows = useMemo(() => actionRows(overrides), [overrides])
  const [editing, setEditing] = useState<string | null>(null)
  const [pending, setPending] = useState<Pending | null>(null)
  const [invalid, setInvalid] = useState(false)

  const commit = (next: Overrides) => {
    const body = overridesPatch(overrides, next)
    if (body) patch.mutate({ shortcuts: body })
  }
  const stop = () => {
    setEditing(null)
    setPending(null)
    setInvalid(false)
  }

  // while a row is waiting for its key: swallow every key (capture phase) so the app's own shortcuts stay quiet
  useEffect(() => {
    if (!editing) return
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault()
      e.stopPropagation()
      e.stopImmediatePropagation()
      if (e.isComposing || e.repeat || MODIFIER_KEYS.has(e.key)) return
      if (e.key === 'Escape') return stop()
      const plan = planAssign(overrides, editing, eventToSpec(e))
      if (plan.kind === 'invalid') return setInvalid(true)
      if (plan.kind === 'same') return stop()
      if (plan.kind === 'ok') {
        commit(plan.overrides)
        return stop()
      }
      setInvalid(false)
      setPending({ id: editing, chord: eventToSpec(e), with: plan.with, swap: plan.swap })
      setEditing(null)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing, overrides])

  const label = (r: Pick<ActionRow, 'id' | 'desc'>) => actionLabel(r.id, r.desc, (k) => t(k))
  const scopeText = (scopes: readonly string[]) => scopes.map((x) => t(`scope.${x}`)).join(' / ')
  const hasOverrides = Object.keys(overrides).length > 0

  return (
    <div className="flex flex-col gap-4" data-testid="settings-keys">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="min-w-0 flex-1 text-xs text-muted">{t('settings.keys.hint')}</p>
        <button
          className="btn"
          disabled={!hasOverrides}
          onClick={() => {
            stop()
            commit({})
          }}
          data-testid="keys-reset-all"
        >
          <RotateCcw size={14} aria-hidden /> {t('settings.keys.resetAll')}
        </button>
      </div>
      {GROUP_ORDER.map((group) => {
        const list = rows.filter((r) => r.group === group)
        if (list.length === 0) return null
        return (
          <Card key={group} title={t(`keyGroup.${group}`)}>
            <table className="w-full">
              <tbody>
                {list.map((r) => {
                  const other = pending?.id === r.id ? pending : null
                  return (
                    <RowView
                      key={r.id}
                      row={r}
                      name={label(r)}
                      scopeHint={r.scopes.length === 1 ? `(${t(`scope.${r.scopes[0]}`)})` : ''}
                      editing={editing === r.id}
                      invalid={invalid && editing === r.id}
                      disabled={!s}
                      onEdit={() => {
                        setPending(null)
                        setInvalid(false)
                        setEditing(editing === r.id ? null : r.id)
                      }}
                      onReset={() => {
                        stop()
                        commit(resetOne(overrides, r.id))
                      }}
                    >
                      {other && (
                        <Conflict
                          pending={other}
                          label={label}
                          scopeText={scopeText}
                          rows={rows}
                          onSwap={() => {
                            if (other.swap) commit(other.swap)
                            stop()
                          }}
                          onCancel={stop}
                        />
                      )}
                    </RowView>
                  )
                })}
              </tbody>
            </table>
          </Card>
        )
      })}
    </div>
  )
}

function RowView(props: {
  row: ActionRow
  name: string
  scopeHint: string
  editing: boolean
  invalid: boolean
  disabled: boolean
  onEdit: () => void
  onReset: () => void
  children?: ReactNode
}) {
  const { t } = useTranslation()
  const { row: r } = props
  return (
    <>
      <tr className="border-b border-line last:border-0" data-testid={`key-row-${r.id}`}>
        <td className="px-4 py-1.5">
          {props.name}
          {props.scopeHint && <span className="ml-1.5 text-xs text-faint">{props.scopeHint}</span>}
          {r.overridden && <span className="ml-1.5 text-xs text-accent-text">{t('settings.keys.custom')}</span>}
        </td>
        <td className="px-4 py-1.5 text-right">
          <span className="inline-flex items-center justify-end gap-2">
            {r.overridden && (
              <button
                className="btn btn-icon btn-ghost"
                onClick={props.onReset}
                title={t('settings.keys.reset', { key: formatSpec(r.defaultChord).join('+') })}
                aria-label={`${t('settings.keys.reset', { key: formatSpec(r.defaultChord).join('+') })}: ${props.name}`}
                data-testid={`key-reset-${r.id}`}
              >
                <RotateCcw size={14} aria-hidden />
              </button>
            )}
            {r.fixed ? (
              <span className="inline-flex items-center gap-1.5" title={t('settings.keys.fixedHint')} data-testid={`key-fixed-${r.id}`}>
                <Lock size={12} className="text-faint" aria-hidden />
                <Chord spec={r.chord} />
              </span>
            ) : (
              <button
                className={`btn ${props.editing ? 'btn-primary' : ''}`}
                disabled={props.disabled}
                aria-pressed={props.editing}
                aria-label={`${props.name}: ${formatSpec(r.chord).join('+')}`}
                onClick={props.onEdit}
                onBlur={() => props.editing && props.onEdit()}
                data-testid={`key-edit-${r.id}`}
              >
                {props.editing ? t('settings.keys.press') : <Chord spec={r.chord} />}
              </button>
            )}
          </span>
          {props.invalid && (
            <p className="mt-1 text-xs text-danger" role="alert">
              {t('settings.keys.invalid')}
            </p>
          )}
        </td>
      </tr>
      {props.children && (
        <tr className="border-b border-line bg-elevated last:border-0">
          <td colSpan={2} className="px-4 py-2">
            {props.children}
          </td>
        </tr>
      )}
    </>
  )
}

function Conflict(props: {
  pending: Pending
  label: (r: Pick<ActionRow, 'id' | 'desc'>) => string
  scopeText: (scopes: readonly string[]) => string
  rows: ActionRow[]
  onSwap: () => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const { pending } = props
  const first = pending.with[0]
  const fixed = pending.with.some((b) => b.fixed)
  const names = [...new Set(pending.with.map((b) => b.id))].map((id) => {
    const b = pending.with.find((x) => x.id === id)!
    return props.label({ id, desc: b.desc })
  })
  return (
    <div className="flex flex-wrap items-center justify-between gap-2" role="alert" data-testid="key-conflict">
      <span className="min-w-0 flex-1 text-sm">
        {t('settings.keys.conflict', {
          key: formatSpec(pending.chord).join('+'),
          action: names.join(', '),
          scope: props.scopeText(first.scopes),
        })}{' '}
        <span className="text-muted">{fixed ? t('settings.keys.conflictFixed') : pending.swap ? t('settings.keys.swapHint') : t('settings.keys.noSwap')}</span>
      </span>
      <span className="flex gap-2">
        {pending.swap && (
          <button className="btn btn-primary" onClick={props.onSwap} data-testid="key-swap">
            {t('settings.keys.swap')}
          </button>
        )}
        <button className="btn" onClick={props.onCancel} data-testid="key-cancel">
          {t('settings.keys.cancel')}
        </button>
      </span>
    </div>
  )
}
