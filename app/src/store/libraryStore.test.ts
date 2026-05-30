import { afterEach, describe, expect, it } from 'vitest';

import type { Collection, LibraryFile, RomEntry } from './libraryStore';
import {
  selectCollectionById,
  selectCollections,
  selectRecentlyPlayed,
  selectRomById,
  selectRoms,
  selectRomsInCollection,
  useLibraryStore,
} from './libraryStore';

const makeRom = (overrides: Partial<RomEntry> = {}): RomEntry => ({
  id: 'rom-1',
  title: 'Test ROM',
  path: '/tmp/test.nes',
  sha1: 'aa'.repeat(20),
  mapper: 0,
  size_bytes: 24_576,
  imported_at: 1_700_000_000_000,
  ...overrides,
});

const makeCollection = (overrides: Partial<Collection> = {}): Collection => ({
  id: 'col-1',
  name: 'Favorites',
  rom_ids: [],
  created_at: 1_700_000_000_000,
  ...overrides,
});

const makeLibrary = (overrides: Partial<LibraryFile> = {}): LibraryFile => ({
  version: 1,
  roms: [],
  collections: [],
  ...overrides,
});

afterEach(() => {
  useLibraryStore.getState().reset();
});

describe('libraryStore reducers', () => {
  it('starts in an empty default state', () => {
    const state = useLibraryStore.getState();
    expect(state.library).toBeNull();
    expect(state.lastPlayedAt).toEqual({});
    expect(state.loading).toBe(false);
    expect(state.error).toBeNull();
  });

  it('setLibrary replaces the snapshot and clears the error', () => {
    useLibraryStore.getState().setError('previous failure');
    const lib = makeLibrary({ roms: [makeRom()] });
    useLibraryStore.getState().setLibrary(lib);
    const state = useLibraryStore.getState();
    expect(state.library).toEqual(lib);
    expect(state.error).toBeNull();
  });

  it('setLoading and setError toggle their respective slices only', () => {
    useLibraryStore.getState().setLoading(true);
    expect(useLibraryStore.getState().loading).toBe(true);
    useLibraryStore.getState().setError('boom');
    expect(useLibraryStore.getState().error).toBe('boom');
    expect(useLibraryStore.getState().loading).toBe(true);
  });

  it('markPlayed records a timestamp per ROM', () => {
    useLibraryStore.getState().markPlayed('rom-1', 10);
    useLibraryStore.getState().markPlayed('rom-2', 20);
    useLibraryStore.getState().markPlayed('rom-1', 30);
    expect(useLibraryStore.getState().lastPlayedAt).toEqual({
      'rom-1': 30,
      'rom-2': 20,
    });
  });

  it('upsertRom inserts a new ROM and updates an existing one', () => {
    useLibraryStore.getState().setLibrary(makeLibrary());
    useLibraryStore.getState().upsertRom(makeRom({ id: 'a', title: 'A' }));
    useLibraryStore.getState().upsertRom(makeRom({ id: 'b', title: 'B' }));
    useLibraryStore.getState().upsertRom(makeRom({ id: 'a', title: 'A-updated' }));
    const roms = selectRoms(useLibraryStore.getState());
    expect(roms.map((r) => r.id)).toEqual(['a', 'b']);
    expect(roms.find((r) => r.id === 'a')?.title).toBe('A-updated');
  });

  it('removeRom drops the ROM, scrubs it from collections, and forgets lastPlayed', () => {
    const library = makeLibrary({
      roms: [makeRom({ id: 'a' }), makeRom({ id: 'b' })],
      collections: [makeCollection({ id: 'c', rom_ids: ['a', 'b'] })],
    });
    useLibraryStore.getState().setLibrary(library);
    useLibraryStore.getState().markPlayed('a', 1);
    useLibraryStore.getState().removeRom('a');

    const state = useLibraryStore.getState();
    expect(state.library?.roms.map((r) => r.id)).toEqual(['b']);
    expect(state.library?.collections[0]?.rom_ids).toEqual(['b']);
    expect(state.lastPlayedAt).not.toHaveProperty('a');
  });

  it('upsertCollection inserts and updates by id', () => {
    useLibraryStore.getState().setLibrary(makeLibrary());
    useLibraryStore.getState().upsertCollection(makeCollection({ id: 'c1', name: 'First' }));
    useLibraryStore.getState().upsertCollection(makeCollection({ id: 'c1', name: 'First renamed' }));
    const cols = selectCollections(useLibraryStore.getState());
    expect(cols).toHaveLength(1);
    expect(cols[0]?.name).toBe('First renamed');
  });

  it('removeCollection drops the collection by id', () => {
    const library = makeLibrary({
      collections: [makeCollection({ id: 'c1' }), makeCollection({ id: 'c2' })],
    });
    useLibraryStore.getState().setLibrary(library);
    useLibraryStore.getState().removeCollection('c1');
    expect(selectCollections(useLibraryStore.getState()).map((c) => c.id)).toEqual(['c2']);
  });
});

describe('libraryStore selectors', () => {
  it('return empty arrays before any library is loaded', () => {
    const state = useLibraryStore.getState();
    expect(selectRoms(state)).toEqual([]);
    expect(selectCollections(state)).toEqual([]);
    expect(selectRomById('anything')(state)).toBeUndefined();
    expect(selectCollectionById('anything')(state)).toBeUndefined();
    expect(selectRomsInCollection('anything')(state)).toEqual([]);
  });

  it('selectRomById and selectCollectionById find by id', () => {
    const a = makeRom({ id: 'a' });
    const c = makeCollection({ id: 'c1', rom_ids: ['a'] });
    useLibraryStore.getState().setLibrary(makeLibrary({ roms: [a], collections: [c] }));
    const state = useLibraryStore.getState();
    expect(selectRomById('a')(state)).toEqual(a);
    expect(selectCollectionById('c1')(state)).toEqual(c);
  });

  it('selectRomsInCollection returns the ROM objects in the collection', () => {
    const a = makeRom({ id: 'a' });
    const b = makeRom({ id: 'b' });
    const c = makeRom({ id: 'c' });
    const col = makeCollection({ id: 'col', rom_ids: ['a', 'c'] });
    useLibraryStore
      .getState()
      .setLibrary(makeLibrary({ roms: [a, b, c], collections: [col] }));
    const result = selectRomsInCollection('col')(useLibraryStore.getState());
    expect(result.map((r) => r.id)).toEqual(['a', 'c']);
  });

  it('selectRecentlyPlayed sorts by lastPlayedAt then imported_at', () => {
    const a = makeRom({ id: 'a', imported_at: 1 });
    const b = makeRom({ id: 'b', imported_at: 2 });
    const c = makeRom({ id: 'c', imported_at: 3 });
    useLibraryStore.getState().setLibrary(makeLibrary({ roms: [a, b, c] }));
    useLibraryStore.getState().markPlayed('a', 100);
    useLibraryStore.getState().markPlayed('b', 200);
    const ordered = selectRecentlyPlayed(useLibraryStore.getState()).map((r) => r.id);
    // b (played 200), a (played 100), c (never played, imported_at 3)
    expect(ordered).toEqual(['b', 'a', 'c']);
  });
});
