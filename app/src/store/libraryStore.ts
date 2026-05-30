// Library client-side state store.
//
// This module owns the *cached* view of the ROM library and collections that
// the UI renders from. It is a pure-reducer-style zustand store: every action
// is a synchronous mutation of state and does **not** invoke IPC. The IPC
// layer (added in a later workflow step) is responsible for calling actions
// here after a successful round-trip to the Tauri host.
//
// Keeping the store IPC-free makes it trivially unit-testable and means we
// can stub the entire backend out for component tests.

import { create } from 'zustand';

/** Stable identifiers minted on the Rust side. */
export type RomId = string;
export type CollectionId = string;

/** Mirrors `RomEntry` from `app/src-tauri/src/library.rs`. */
export interface RomEntry {
  id: RomId;
  title: string;
  path: string;
  sha1: string;
  mapper: number;
  size_bytes: number;
  imported_at: number;
}

/** Mirrors `Collection` from `app/src-tauri/src/library.rs`. */
export interface Collection {
  id: CollectionId;
  name: string;
  rom_ids: RomId[];
  created_at: number;
}

/** Mirrors `LibraryFile` from `app/src-tauri/src/library.rs`. */
export interface LibraryFile {
  version: number;
  roms: RomEntry[];
  collections: Collection[];
}

export interface LibraryState {
  /** Last library snapshot received from the backend, or `null` before load. */
  library: LibraryFile | null;
  /** Local-only timestamps tracking when a ROM was last played (FR-9). */
  lastPlayedAt: Record<RomId, number>;
  /** Truthy while a list/refresh round-trip is in flight. */
  loading: boolean;
  /** Last error message surfaced from the backend, or `null`. */
  error: string | null;

  // Reducer-style actions ---------------------------------------------------
  setLibrary: (library: LibraryFile) => void;
  setLoading: (loading: boolean) => void;
  setError: (error: string | null) => void;
  markPlayed: (romId: RomId, at?: number) => void;
  upsertRom: (rom: RomEntry) => void;
  removeRom: (id: RomId) => void;
  upsertCollection: (collection: Collection) => void;
  removeCollection: (id: CollectionId) => void;
  reset: () => void;
}

const EMPTY_LIBRARY: LibraryFile = {
  version: 1,
  roms: [],
  collections: [],
};

const DEFAULT_STATE: Pick<LibraryState, 'library' | 'lastPlayedAt' | 'loading' | 'error'> = {
  library: null,
  lastPlayedAt: {},
  loading: false,
  error: null,
};

export const useLibraryStore = create<LibraryState>((set) => ({
  ...DEFAULT_STATE,

  setLibrary: (library) => set({ library, error: null }),

  setLoading: (loading) => set({ loading }),

  setError: (error) => set({ error }),

  markPlayed: (romId, at = Date.now()) =>
    set((state) => ({
      lastPlayedAt: { ...state.lastPlayedAt, [romId]: at },
    })),

  upsertRom: (rom) =>
    set((state) => {
      const current = state.library ?? EMPTY_LIBRARY;
      const idx = current.roms.findIndex((r) => r.id === rom.id);
      const roms =
        idx === -1
          ? [...current.roms, rom]
          : current.roms.map((r, i) => (i === idx ? rom : r));
      return { library: { ...current, roms } };
    }),

  removeRom: (id) =>
    set((state) => {
      if (!state.library) return state;
      const roms = state.library.roms.filter((r) => r.id !== id);
      const collections = state.library.collections.map((c) => ({
        ...c,
        rom_ids: c.rom_ids.filter((rid) => rid !== id),
      }));
      // Also forget the lastPlayed timestamp.
      const lastPlayedAt = { ...state.lastPlayedAt };
      delete lastPlayedAt[id];
      return {
        library: { ...state.library, roms, collections },
        lastPlayedAt,
      };
    }),

  upsertCollection: (collection) =>
    set((state) => {
      const current = state.library ?? EMPTY_LIBRARY;
      const idx = current.collections.findIndex((c) => c.id === collection.id);
      const collections =
        idx === -1
          ? [...current.collections, collection]
          : current.collections.map((c, i) => (i === idx ? collection : c));
      return { library: { ...current, collections } };
    }),

  removeCollection: (id) =>
    set((state) => {
      if (!state.library) return state;
      const collections = state.library.collections.filter((c) => c.id !== id);
      return { library: { ...state.library, collections } };
    }),

  reset: () => set({ ...DEFAULT_STATE }),
}));

// -------------------------------------------------------------------------
// Selectors. Defined as plain functions so they can be unit-tested against
// a snapshot without rendering React.
// -------------------------------------------------------------------------

export const selectRoms = (state: LibraryState): RomEntry[] => state.library?.roms ?? [];

export const selectCollections = (state: LibraryState): Collection[] =>
  state.library?.collections ?? [];

export const selectRomById =
  (id: RomId) =>
  (state: LibraryState): RomEntry | undefined =>
    state.library?.roms.find((r) => r.id === id);

export const selectCollectionById =
  (id: CollectionId) =>
  (state: LibraryState): Collection | undefined =>
    state.library?.collections.find((c) => c.id === id);

export const selectRomsInCollection =
  (id: CollectionId) =>
  (state: LibraryState): RomEntry[] => {
    if (!state.library) return [];
    const collection = state.library.collections.find((c) => c.id === id);
    if (!collection) return [];
    const ids = new Set(collection.rom_ids);
    return state.library.roms.filter((r) => ids.has(r.id));
  };

/** Roms sorted by most-recently-played, then most-recently-imported. */
export const selectRecentlyPlayed = (state: LibraryState): RomEntry[] => {
  const roms = state.library?.roms ?? [];
  const played = state.lastPlayedAt;
  return [...roms].sort((a, b) => {
    const pa = played[a.id] ?? 0;
    const pb = played[b.id] ?? 0;
    if (pa !== pb) return pb - pa;
    return b.imported_at - a.imported_at;
  });
};
