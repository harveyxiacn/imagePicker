import { useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { Modal } from '../Modal'

/** A titled card holding setting rows. */
export function Card({ title, hint, children, testId }: { title?: string; hint?: string; children: ReactNode; testId?: string }) {
  return (
    <section className="rounded-card border border-line bg-panel" data-testid={testId}>
      {(title || hint) && (
        <header className="border-b border-line px-4 py-2.5">
          {title && <h3 className="text-base font-semibold">{title}</h3>}
          {hint && <p className="mt-0.5 text-xs text-muted">{hint}</p>}
        </header>
      )}
      <div className="flex flex-col divide-y divide-line">{children}</div>
    </section>
  )
}

/** Label + description on the left, control on the right (wraps on phones). */
export function Row({ label, hint, children, testId }: { label: string; hint?: ReactNode; children?: ReactNode; testId?: string }) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-2 px-4 py-3" data-testid={testId}>
      <div className="min-w-0 flex-1 basis-60">
        <div className="font-medium">{label}</div>
        {hint && <div className="mt-0.5 text-xs text-muted">{hint}</div>}
      </div>
      {children !== undefined && <div className="flex shrink-0 items-center gap-2">{children}</div>}
    </div>
  )
}

export function Toggle({ checked, onChange, label, disabled, testId }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean; testId?: string }) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      data-testid={testId}
      className={`relative h-5 w-9 shrink-0 rounded-full border transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${checked ? 'border-accent bg-accent' : 'border-line bg-bg'}`}
    >
      <span className={`absolute top-0.5 h-3.5 w-3.5 rounded-full transition-[left] ${checked ? 'left-[18px] bg-accent-fg' : 'left-0.5 bg-muted'}`} />
    </button>
  )
}

export function Select<T extends string>({
  value,
  options,
  onChange,
  label,
  testId,
  disabled,
}: {
  value: T
  options: { value: T; label: string }[]
  onChange: (v: T) => void
  label: string
  testId?: string
  disabled?: boolean
}) {
  return (
    <select className="field" value={value} aria-label={label} data-testid={testId} disabled={disabled} onChange={(e) => onChange(e.target.value as T)}>
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  )
}

/** Text / number input that commits on blur or Enter, validating through `parse` (null = invalid, shows red). */
export function CommitInput<T>(props: CommitInputProps<T>) {
  // remount when the committed value changes so the draft text resets
  return <CommitInputInner key={props.format(props.value)} {...props} />
}

interface CommitInputProps<T> {
  value: T
  format: (v: T) => string
  parse: (raw: string) => T | null
  onCommit: (v: T) => void
  label: string
  width?: string
  suffix?: string
  testId?: string
  type?: 'text' | 'number'
}

function CommitInputInner<T>({
  value,
  format,
  parse,
  onCommit,
  label,
  width = 'w-24',
  suffix,
  testId,
  type = 'text',
}: CommitInputProps<T>) {
  const [raw, setRaw] = useState(format(value))
  const parsed = parse(raw)
  const invalid = parsed === null
  const commit = () => {
    if (parsed === null) setRaw(format(value))
    else if (format(parsed) !== format(value)) onCommit(parsed)
  }
  return (
    <span className="flex items-center gap-1.5">
      <input
        className={`field ${width} text-right tnum ${invalid ? '!border-danger' : ''}`}
        type={type}
        value={raw}
        aria-label={label}
        aria-invalid={invalid}
        data-testid={testId}
        onChange={(e) => setRaw(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit()
        }}
      />
      {suffix && <span className="text-muted">{suffix}</span>}
    </span>
  )
}

export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  onConfirm,
  danger,
  children,
  confirmDisabled,
  testId,
}: {
  open: boolean
  onOpenChange: (b: boolean) => void
  title: string
  description?: string
  confirmLabel: string
  onConfirm: () => void
  danger?: boolean
  children?: ReactNode
  confirmDisabled?: boolean
  testId?: string
}) {
  const { t } = useTranslation()
  return (
    <Modal
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      description={description}
      width="max-w-md"
      footer={
        <>
          <button className="btn" onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </button>
          <button className={`btn ${danger ? 'border-danger bg-danger text-white' : 'btn-primary'}`} disabled={confirmDisabled} onClick={onConfirm} data-testid={testId}>
            {confirmLabel}
          </button>
        </>
      }
    >
      {children}
    </Modal>
  )
}
