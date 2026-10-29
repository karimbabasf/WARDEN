// catalog.ts: the sets a bench can build, all packed offline under public/brickyard
// by scripts/brickyard/pack.mjs. A bench's nth set is a pure function of its agent id,
// so the same session always builds the same sets in the same order.

export type SetEntry = {
  /** The packed file under /brickyard/. */
  file: string;
  number: string;
  name: string;
  /** Release year, only where it is known for certain. */
  year?: number;
};

export const SET_CATALOG: readonly SetEntry[] = [
  { file: '60001-1.mpd', number: '60001', name: 'Fire Rescue', year: 2013 },
  { file: '6350-1.mpd', number: '6350', name: 'Pizza To Go', year: 1994 },
  { file: '7130-1.mpd', number: '7130', name: 'Snowspeeder', year: 1999 },
  { file: '21013-1.mpd', number: '21013', name: 'Big Ben', year: 2012 },
  { file: '6040-1.mpd', number: '6040', name: 'Blacksmith Shop', year: 1984 },
  { file: '6890-1.mpd', number: '6890', name: 'Cosmic Cruiser' },
  { file: '6365-1.mpd', number: '6365', name: 'Summer Cottage' },
  { file: '40182-1.mpd', number: '40182', name: 'Bricktober Fire Station' },
  { file: '75892-1.mpd', number: '75892', name: 'McLaren Senna', year: 2019 },
  { file: '40143-1.mpd', number: '40143', name: 'Bricktober Bakery' },
  { file: '6388-1.mpd', number: '6388', name: 'Holiday Home with Caravan' },
  { file: '7110-1.mpd', number: '7110', name: 'Landspeeder', year: 1999 },
];

/** FNV-1a: small, stable across runs and engines, good enough to spread ids. */
export function hashId(id: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < id.length; i++) {
    h ^= id.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** The nth set (0-based) a bench builds. Consecutive sets never repeat back to back. */
export function setFor(agentId: string, n: number, catalog: readonly SetEntry[] = SET_CATALOG, offset = 0): SetEntry {
  return catalog[(hashId(agentId) + offset + n) % catalog.length];
}

/**
 * Give every bench its own set: each bench starts at its hashed place in the catalog
 * and steps forward past any start another bench already holds. Benches are taken in
 * start order, so an older bench never changes set when a new one opens.
 */
export function distinctOffsets(ids: string[], size = SET_CATALOG.length): Map<string, number> {
  const taken = new Set<number>();
  const out = new Map<string, number>();
  for (const id of ids) {
    let off = 0;
    while (off < size && taken.has((hashId(id) + off) % size)) off++;
    taken.add((hashId(id) + (off % size)) % size);
    out.set(id, off % size);
  }
  return out;
}

export const assetUrl = (file: string): string => `/brickyard/${file}`;
