/**
 * Display names for people. Stored ids are database cluster ids with gaps (2, 1, 91, ...) and must
 * never reach the UI; unnamed people are numbered 1..N in the order the API lists them instead.
 * The stored name / id are untouched, so renaming and filtering keep working on real ids.
 */
export interface NameRef {
  id: number
  name: string | null
}

/** id -> 1-based number for the unnamed people, in list order. Named people have no entry. */
export function unnamedNumbers(people: readonly NameRef[]): Map<number, number> {
  const out = new Map<number, number>()
  for (const p of people) if (p.name === null || p.name.trim() === '') out.set(p.id, out.size + 1)
  return out
}

/** Attach `unnamed_no` to every unnamed person (used once, in the people query's select). */
export function withUnnamedNumbers<T extends NameRef>(people: readonly T[]): (T & { unnamed_no?: number })[] {
  const nums = unnamedNumbers(people)
  return people.map((p) => (nums.has(p.id) ? { ...p, unnamed_no: nums.get(p.id) } : p))
}

/** The one place that turns a person into text: real name, else "Unnamed N", else bare "Unnamed". */
export function displayName(p: { name: string | null; unnamed_no?: number }, unnamed: string): string {
  if (p.name !== null && p.name.trim() !== '') return p.name
  return p.unnamed_no ? `${unnamed} ${p.unnamed_no}` : unnamed
}
