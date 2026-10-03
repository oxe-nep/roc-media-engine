/** Stable channel order: always sort by numeric id (index). */
export function sortByChannelId<T extends { id: number }>(items: readonly T[]): T[] {
  return [...items].sort((a, b) => a.id - b.id);
}
