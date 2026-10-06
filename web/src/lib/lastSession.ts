const KEY = 'imagepicker.lastSession'

/** Last opened library session (the bottom nav targets it from pages without a session in the URL). */
export function rememberSession(id: number): void {
  try {
    localStorage.setItem(KEY, String(id))
  } catch {
    /* storage blocked */
  }
}

export function lastSession(): number | null {
  try {
    const n = Number(localStorage.getItem(KEY))
    return Number.isInteger(n) && n > 0 ? n : null
  } catch {
    return null
  }
}
