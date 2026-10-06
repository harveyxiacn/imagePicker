import type { ReactNode } from 'react'

export function MenuItem({ icon, children, disabled, onClick }: { icon: ReactNode; children: ReactNode; disabled?: boolean; onClick: () => void }) {
  return (
    <button
      role="menuitem"
      className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left hover:bg-panel disabled:cursor-not-allowed disabled:opacity-40"
      disabled={disabled}
      onClick={onClick}
    >
      {icon}
      {children}
    </button>
  )
}
