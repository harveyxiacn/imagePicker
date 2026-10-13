import type { Scope } from './keymap'

/**
 * The screen that currently owns keyboard actions (library or edit page) registers its handlers here so the
 * command palette can list and run them. Pages without keyboard actions (home, settings...) register nothing and
 * the palette only offers navigation there.
 */
export interface PaletteHost {
  scope: () => Scope
  has: (id: string) => boolean
  run: (id: string) => void
}

let current: PaletteHost | null = null

export function registerPaletteHost(host: PaletteHost): () => void {
  current = host
  return () => {
    if (current === host) current = null
  }
}

export function getPaletteHost(): PaletteHost | null {
  return current
}
