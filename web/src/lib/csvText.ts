/**
 * Comma separated text values for query params (`lens`, `tags`): names may contain commas, so `,` is
 * written `\,` and `\` is written `\\` (mirrors `csv_text` in ip-server routes).
 */
export function joinCsvText(items: readonly string[]): string {
  return items.map((s) => s.replace(/\\/g, '\\\\').replace(/,/g, '\\,')).join(',')
}

/** Inverse of {@link joinCsvText}; blank items are dropped, the rest is trimmed. */
export function splitCsvText(v: string | null | undefined): string[] {
  const out: string[] = []
  let cur = ''
  for (let i = 0; i < (v ?? '').length; i++) {
    const c = v![i]
    if (c === '\\' && i + 1 < v!.length) cur += v![++i]
    else if (c === ',') {
      out.push(cur)
      cur = ''
    } else cur += c
  }
  out.push(cur)
  return out.map((s) => s.trim()).filter(Boolean)
}
