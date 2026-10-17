// Ranks docs/search.json entries for a query. Shared by the ⌘K palette and the docs MCP server.
const fold = text => text.toLowerCase().normalize('NFKD').replace(/[̀-ͯ]/g, '');

export function rank(index, query, limit = 12) {
  const words = fold(query).split(/[^a-z0-9_]+/).filter(Boolean);
  if (!words.length) return [];
  const phrase = fold(query).trim();
  const scored = [];
  for (const entry of index) {
    const title = fold(entry.t);
    const page = fold(entry.p || '');
    const text = fold(entry.x || '');
    let score = 0;
    for (const word of words) {
      const inTitle = title.includes(word);
      const inPage = page.includes(word);
      const inText = text.includes(word);
      if (!inTitle && !inPage && !inText) { score = 0; break; }
      score += inTitle ? (new RegExp(`(^|[^a-z0-9])${word.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`).test(title) ? 12 : 8) : 0;
      score += inPage ? 3 : 0;
      score += inText ? 2 : 0;
    }
    if (!score) continue;
    if (title === phrase) score += 30;
    else if (title.startsWith(phrase)) score += 14;
    else if (text.includes(phrase) && words.length > 1) score += 6;
    if (!entry.p) score += 4; // whole pages edge out their own sections
    scored.push({ entry, score });
  }
  return scored.sort((a, b) => b.score - a.score).slice(0, limit).map(({ entry }) => entry);
}
