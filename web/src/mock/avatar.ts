// Synthetic SVG faces for the mock backend. Open/closed eyes and smile depth are visible so the
// expression matrix, face strips and people cards can be reviewed visually.

export interface AvatarStyle {
  skin: string
  hair: string
  /** 0 short, 1 long, 2 bun, 3 cap */
  hairStyle: 0 | 1 | 2 | 3
  hue: number
}

export const AVATAR_STYLES: AvatarStyle[] = [
  { skin: '#f2c9a5', hair: '#2b1d14', hairStyle: 0, hue: 28 },
  { skin: '#f6d2b8', hair: '#1a1210', hairStyle: 1, hue: 340 },
  { skin: '#d9a77c', hair: '#0f0f0f', hairStyle: 0, hue: 205 },
  { skin: '#f0c0a0', hair: '#6b4a3a', hairStyle: 2, hue: 150 },
  { skin: '#e2b08a', hair: '#3a3a3a', hairStyle: 3, hue: 260 },
  { skin: '#f7d9c4', hair: '#a8552f', hairStyle: 1, hue: 40 },
  { skin: '#cf9a6e', hair: '#d8d8d8', hairStyle: 0, hue: 95 },
  { skin: '#efc3a2', hair: '#14100c', hairStyle: 1, hue: 310 },
  { skin: '#e5b894', hair: '#4a2f1d', hairStyle: 0, hue: 180 },
  { skin: '#f4cfb0', hair: '#22160f', hairStyle: 2, hue: 15 },
]

const f = (n: number) => n.toFixed(1)

/** Face drawing in a 100x100 box (no background). */
export function faceInner(st: AvatarStyle, eyesOpen: number, smile: number): string {
  const dark = '#2a1b14'
  const parts: string[] = []
  // hair behind head
  if (st.hairStyle === 1) parts.push(`<rect x="19" y="30" width="62" height="58" rx="26" fill="${st.hair}"/>`)
  if (st.hairStyle === 2) parts.push(`<circle cx="50" cy="12" r="11" fill="${st.hair}"/>`)
  // head
  parts.push(`<ellipse cx="50" cy="54" rx="28" ry="33" fill="${st.skin}"/>`)
  // hair on top
  parts.push(`<path d="M21 46 Q20 16 50 16 Q80 16 79 46 Q70 30 50 30 Q30 30 21 46Z" fill="${st.hair}"/>`)
  if (st.hairStyle === 3) {
    parts.push(`<path d="M18 36 Q50 6 82 36 L82 40 Q50 34 18 40Z" fill="#3b6ea5"/><rect x="50" y="34" width="38" height="5" rx="2" fill="#2f5887"/>`)
  }
  // eyebrows
  parts.push(`<path d="M30 41 Q38 37 45 41M55 41 Q62 37 70 41" stroke="${dark}" stroke-width="2.2" fill="none" stroke-linecap="round"/>`)
  // eyes
  const closed = eyesOpen < 0.45
  for (const cx of [38, 62]) {
    if (closed) {
      parts.push(`<path d="M${cx - 7} 50 Q${cx} ${f(50 + 5 - eyesOpen * 8)} ${cx + 7} 50" stroke="${dark}" stroke-width="2.4" fill="none" stroke-linecap="round"/>`)
    } else {
      const ry = Math.max(1.4, eyesOpen * 6)
      parts.push(
        `<ellipse cx="${cx}" cy="50" rx="7" ry="${f(ry)}" fill="#fff" stroke="${dark}" stroke-width="1.2"/>` +
          `<circle cx="${cx}" cy="50" r="${f(Math.min(3.4, ry - 0.2))}" fill="#3a2a22"/>`,
      )
    }
  }
  // nose
  parts.push(`<path d="M50 52 Q47 61 51 62" stroke="#00000033" stroke-width="1.6" fill="none" stroke-linecap="round"/>`)
  // mouth: control point drops with smile; wide smiles show teeth
  const cy = 70 + (smile - 0.25) * 34
  const peak = 70 + (cy - 70) / 2
  if (smile > 0.62) {
    parts.push(`<path d="M36 69 Q50 ${f(cy)} 64 69 Q50 ${f(peak - 1)} 36 69Z" fill="#fff" stroke="#8a2f2f" stroke-width="1.8" stroke-linejoin="round"/>`)
  } else {
    parts.push(`<path d="M38 70 Q50 ${f(cy)} 62 70" stroke="#8a2f2f" stroke-width="2.4" fill="none" stroke-linecap="round"/>`)
  }
  return parts.join('')
}

export function avatarSvg(st: AvatarStyle, eyesOpen: number, smile: number, size: number): string {
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 100 100">` +
    `<defs><linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${st.hue} 35% 30%)"/>` +
    `<stop offset="1" stop-color="hsl(${(st.hue + 40) % 360} 40% 18%)"/></linearGradient></defs>` +
    `<rect width="100" height="100" fill="url(#bg)"/>` +
    `<g transform="translate(0 6) scale(.94) translate(3 0)">${faceInner(st, eyesOpen, smile)}</g></svg>`
  )
}
