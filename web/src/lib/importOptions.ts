/** One exclude pattern per line (globs, sub-folders or names); blank lines and duplicates are dropped. */
export function parseExcludes(text: string): string[] {
  const seen = new Set<string>()
  for (const line of text.split(/\r?\n/)) {
    const s = line.trim()
    if (s) seen.add(s)
  }
  return [...seen]
}
