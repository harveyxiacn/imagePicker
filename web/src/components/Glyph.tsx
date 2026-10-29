/**
 * Badge text drawn by CSS (`.glyph`): visible, but not DOM text. A thumbnail's accessible name is photoLabel(); text
 * rendered inside the thumbnail would otherwise have to be a literal substring of it (WCAG 2.5.3 label in name).
 */
export const Glyph = ({ text }: { text: string }) => <span className="glyph" data-glyph={text} />
