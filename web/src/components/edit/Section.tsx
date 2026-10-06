import { ChevronDown, ChevronRight } from 'lucide-react'
import type { ReactNode } from 'react'
import { useEdit, type PanelId } from '@/stores/edit'

interface Props {
  id: PanelId
  title: string
  icon?: ReactNode
  /** content shown on the right of the header (reset buttons...) */
  right?: ReactNode
  /** purple AI styling */
  ai?: boolean
  /** collapsed unless the user opened it */
  defaultCollapsed?: boolean
  children: ReactNode
}

/** Collapsible panel section (state persists in the edit store). */
export function Section({ id, title, icon, right, ai, defaultCollapsed, children }: Props) {
  const stored = useEdit((s) => s.collapsed[id])
  const collapsed = stored ?? defaultCollapsed ?? false
  const toggle = useEdit((s) => s.togglePanel)
  // `defaultCollapsed` must toggle relative to the effective state.
  const onToggle = () => {
    if (stored === undefined && defaultCollapsed) useEdit.setState((s) => ({ collapsed: { ...s.collapsed, [id]: false } }))
    else toggle(id)
  }
  return (
    <section className="border-b border-line" data-testid={`section-${id}`}>
      <div className="flex h-8 items-center gap-1 px-3">
        <button
          type="button"
          className={`flex min-w-0 flex-1 items-center gap-1.5 text-left text-[12px] font-semibold tracking-wide uppercase ${ai ? 'text-ai' : 'text-muted hover:text-fg'}`}
          aria-expanded={!collapsed}
          onClick={onToggle}
        >
          {collapsed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
          {icon}
          <span className="truncate">{title}</span>
        </button>
        {right}
      </div>
      {!collapsed && <div className="px-3 pb-3">{children}</div>}
    </section>
  )
}
