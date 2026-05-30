// Unit tests for the typed `invoke` wrappers in `./client.ts`.
//
// Each wrapper is exercised with a mocked `@tauri-apps/api/core` so we can
// assert:
// - the wrapper passes its arguments to `invoke` under the expected
//   snake_case command name, and
// - the resolved value is returned to the caller verbatim, and
// - a structured `{ code, details? }` rejection is rethrown unchanged so
//   callers receive the typed `AppError` discriminator.

import { beforeEach, describe, expect, it, vi } from 'vitest';

// IMPORTANT: `vi.mock` is hoisted to the top of the file before all imports.
// The async factory dynamically imports the shim so it can reference
// `invokeMock` / `MockChannel` without running into the hoisting trap.
vi.mock('@tauri-apps/api/core', async () => {
  const shim = await import('./tauri-shim');
  return shim.tauriCoreMockFactory();
});

import {
  addRomToCollection,
  Channel,
  createCollection,
  deleteCollection,
  getSettings,
  importRomFromDialog,
  importRomFromPath,
  listLibrary,
  log,
  openExternal,
  removeRomFromCollection,
  removeRomFromLibrary,
  renameCollection,
  renameRom,
  resetBindings,
  setButtonState,
  setMuted,
  setPaused,
  setVolume,
  startSession,
  stopSession,
  toggleFullscreen,
  updateBindings,
} from './client';
import { invokeMock } from './tauri-shim';
import type {
  Collection,
  FrameMessage,
  FullscreenState,
  LibraryFile,
  RomEntry,
  Settings,
} from './types';
import { APP_ERROR_CODES, isAppError } from './types';

const ROM_ID = '11111111-1111-4111-8111-111111111111';
const COLLECTION_ID = '22222222-2222-4222-8222-222222222222';

const sampleRom = (): RomEntry => ({
  id: ROM_ID,
  title: 'Demo',
  path: '/tmp/demo.nes',
  sha1: 'a'.repeat(40),
  mapper: 0,
  size_bytes: 24576,
  imported_at: 1_700_000_000_000,
});

const sampleCollection = (): Collection => ({
  id: COLLECTION_ID,
  name: 'Favorites',
  rom_ids: [ROM_ID],
  created_at: 1_700_000_001_000,
});

const sampleLibrary = (): LibraryFile => ({
  version: 1,
  roms: [sampleRom()],
  collections: [sampleCollection()],
});

const sampleSettings = (): Settings => ({
  version: 1,
  bindings: {
    p1: {
      up: 'KeyW',
      down: 'KeyS',
      left: 'KeyA',
      right: 'KeyD',
      a: 'KeyJ',
      b: 'KeyK',
      start: 'Enter',
      select: 'ShiftRight',
    },
    p2: {
      up: 'ArrowUp',
      down: 'ArrowDown',
      left: 'ArrowLeft',
      right: 'ArrowRight',
      a: 'Numpad0',
      b: 'NumpadDecimal',
      start: 'NumpadEnter',
      select: 'NumpadAdd',
    },
  },
  volume: 0.75,
  muted: false,
  fullscreen_shortcut: 'F11',
  window: null,
});

beforeEach(() => {
  invokeMock.mockReset();
});

// ---------------------------------------------------------------------------
// Library group
// ---------------------------------------------------------------------------

describe('library wrappers', () => {
  it('listLibrary calls "list_library" with no args and returns the snapshot', async () => {
    const lib = sampleLibrary();
    invokeMock.mockResolvedValueOnce(lib);
    const result = await listLibrary();
    expect(invokeMock).toHaveBeenCalledWith('list_library', undefined);
    expect(result).toEqual(lib);
  });

  it('importRomFromPath forwards the path argument', async () => {
    const rom = sampleRom();
    invokeMock.mockResolvedValueOnce(rom);
    const result = await importRomFromPath('/tmp/demo.nes');
    expect(invokeMock).toHaveBeenCalledWith('import_rom_from_path', {
      path: '/tmp/demo.nes',
    });
    expect(result).toBe(rom);
  });

  it('importRomFromDialog calls "import_rom_from_dialog" and resolves to null on cancel', async () => {
    invokeMock.mockResolvedValueOnce(null);
    const result = await importRomFromDialog();
    expect(invokeMock).toHaveBeenCalledWith(
      'import_rom_from_dialog',
      undefined,
    );
    expect(result).toBeNull();
  });

  it('removeRomFromLibrary forwards the id argument', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await removeRomFromLibrary(ROM_ID);
    expect(invokeMock).toHaveBeenCalledWith(
      'remove_rom_from_library',
      { id: ROM_ID },
    );
  });

  it('renameRom forwards id and title and returns the updated entry', async () => {
    const rom = { ...sampleRom(), title: 'New Title' };
    invokeMock.mockResolvedValueOnce(rom);
    const result = await renameRom(ROM_ID, 'New Title');
    expect(invokeMock).toHaveBeenCalledWith('rename_rom', {
      id: ROM_ID,
      title: 'New Title',
    });
    expect(result).toEqual(rom);
  });

  it('createCollection forwards the name argument', async () => {
    const coll = sampleCollection();
    invokeMock.mockResolvedValueOnce(coll);
    const result = await createCollection('Favorites');
    expect(invokeMock).toHaveBeenCalledWith('create_collection', {
      name: 'Favorites',
    });
    expect(result).toEqual(coll);
  });

  it('renameCollection forwards id and name', async () => {
    const coll = { ...sampleCollection(), name: 'Renamed' };
    invokeMock.mockResolvedValueOnce(coll);
    const result = await renameCollection(COLLECTION_ID, 'Renamed');
    expect(invokeMock).toHaveBeenCalledWith('rename_collection', {
      id: COLLECTION_ID,
      name: 'Renamed',
    });
    expect(result).toEqual(coll);
  });

  it('deleteCollection forwards the id argument', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await deleteCollection(COLLECTION_ID);
    expect(invokeMock).toHaveBeenCalledWith('delete_collection', {
      id: COLLECTION_ID,
    });
  });

  it('addRomToCollection forwards both ids under their snake_case keys', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await addRomToCollection(COLLECTION_ID, ROM_ID);
    expect(invokeMock).toHaveBeenCalledWith('add_rom_to_collection', {
      collection: COLLECTION_ID,
      rom: ROM_ID,
    });
  });

  it('removeRomFromCollection forwards both ids', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await removeRomFromCollection(COLLECTION_ID, ROM_ID);
    expect(invokeMock).toHaveBeenCalledWith(
      'remove_rom_from_collection',
      { collection: COLLECTION_ID, rom: ROM_ID },
    );
  });
});

// ---------------------------------------------------------------------------
// Emulator group
// ---------------------------------------------------------------------------

describe('emulator wrappers', () => {
  it('startSession forwards the rom source and the channel under "frames"', async () => {
    const info = { sha1: 'a'.repeat(40), has_battery: false, mapper: 0 };
    invokeMock.mockResolvedValueOnce(info);
    const channel = new Channel<FrameMessage>();
    const result = await startSession({ kind: 'library', id: ROM_ID }, channel);
    expect(invokeMock).toHaveBeenCalledWith('start_session', {
      rom: { kind: 'library', id: ROM_ID },
      frames: channel,
    });
    expect(result).toEqual(info);
  });

  it('startSession accepts a "path" source', async () => {
    invokeMock.mockResolvedValueOnce({
      sha1: 'b'.repeat(40),
      has_battery: true,
      mapper: 1,
    });
    const channel = new Channel<FrameMessage>();
    await startSession({ kind: 'path', path: '/tmp/x.nes' }, channel);
    expect(invokeMock).toHaveBeenCalledWith('start_session', {
      rom: { kind: 'path', path: '/tmp/x.nes' },
      frames: channel,
    });
  });

  it('stopSession calls "stop_session" with no args', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await stopSession();
    expect(invokeMock).toHaveBeenCalledWith('stop_session', undefined);
  });

  it('setButtonState forwards player, button, and pressed', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await setButtonState(2, 'Start', true);
    expect(invokeMock).toHaveBeenCalledWith('set_button_state', {
      player: 2,
      button: 'Start',
      pressed: true,
    });
  });

  it('setPaused forwards the paused flag', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await setPaused(true);
    expect(invokeMock).toHaveBeenCalledWith('set_paused', {
      paused: true,
    });
  });

  it('setMuted forwards the muted flag', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await setMuted(true);
    expect(invokeMock).toHaveBeenCalledWith('set_muted', {
      muted: true,
    });
  });

  it('setVolume forwards the volume argument', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await setVolume(0.42);
    expect(invokeMock).toHaveBeenCalledWith('set_volume', {
      volume: 0.42,
    });
  });
});

// ---------------------------------------------------------------------------
// Settings group
// ---------------------------------------------------------------------------

describe('settings wrappers', () => {
  it('getSettings calls "get_settings" with no args', async () => {
    const settings = sampleSettings();
    invokeMock.mockResolvedValueOnce(settings);
    const result = await getSettings();
    expect(invokeMock).toHaveBeenCalledWith(
      'get_settings',
      undefined,
    );
    expect(result).toEqual(settings);
  });

  it('updateBindings forwards player and map', async () => {
    const settings = sampleSettings();
    invokeMock.mockResolvedValueOnce(settings);
    const map = settings.bindings.p1;
    const result = await updateBindings(1, map);
    expect(invokeMock).toHaveBeenCalledWith('update_bindings', {
      player: 1,
      map,
    });
    expect(result).toEqual(settings);
  });

  it('resetBindings calls "reset_bindings" with no args', async () => {
    const settings = sampleSettings();
    invokeMock.mockResolvedValueOnce(settings);
    await resetBindings();
    expect(invokeMock).toHaveBeenCalledWith(
      'reset_bindings',
      undefined,
    );
  });
});

// ---------------------------------------------------------------------------
// Shell group
// ---------------------------------------------------------------------------

describe('shell wrappers', () => {
  it('toggleFullscreen returns the new fullscreen flag', async () => {
    const state: FullscreenState = { fullscreen: true };
    invokeMock.mockResolvedValueOnce(state);
    const result = await toggleFullscreen();
    expect(invokeMock).toHaveBeenCalledWith(
      'toggle_fullscreen',
      undefined,
    );
    expect(result).toEqual(state);
  });

  it('openExternal forwards the url argument', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await openExternal('https://example.com');
    expect(invokeMock).toHaveBeenCalledWith('open_external', {
      url: 'https://example.com',
    });
  });

  it('log forwards level and message', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await log('warn', 'something happened');
    expect(invokeMock).toHaveBeenCalledWith('log', {
      level: 'warn',
      message: 'something happened',
    });
  });
});

// ---------------------------------------------------------------------------
// Error decoding
// ---------------------------------------------------------------------------

describe('error decoding', () => {
  it('rethrows a structured InvalidRom error as a typed AppError', async () => {
    invokeMock.mockRejectedValueOnce({
      code: 'InvalidRom',
      details: 'missing NES magic',
    });
    let thrown: unknown = null;
    try {
      await importRomFromPath('/x.nes');
    } catch (err) {
      thrown = err;
    }
    expect(thrown).not.toBeNull();
    expect(isAppError(thrown)).toBe(true);
    if (isAppError(thrown) && thrown.code === 'InvalidRom') {
      expect(thrown.details).toBe('missing NES magic');
    } else {
      throw new Error('expected an InvalidRom AppError');
    }
  });

  it('rethrows a structured UnsupportedMapper error with numeric details', async () => {
    invokeMock.mockRejectedValueOnce({
      code: 'UnsupportedMapper',
      details: 9,
    });
    let thrown: unknown = null;
    try {
      await importRomFromPath('/y.nes');
    } catch (err) {
      thrown = err;
    }
    expect(isAppError(thrown)).toBe(true);
    if (isAppError(thrown) && thrown.code === 'UnsupportedMapper') {
      expect(thrown.details).toBe(9);
    } else {
      throw new Error('expected an UnsupportedMapper AppError');
    }
  });

  it('rethrows a NotFound error without details', async () => {
    invokeMock.mockRejectedValueOnce({ code: 'NotFound' });
    let thrown: unknown = null;
    try {
      await removeRomFromLibrary(ROM_ID);
    } catch (err) {
      thrown = err;
    }
    expect(isAppError(thrown)).toBe(true);
    if (isAppError(thrown)) {
      expect(thrown.code).toBe('NotFound');
    }
  });

  it('rethrows non-structured errors unchanged', async () => {
    const error = new Error('transport failure');
    invokeMock.mockRejectedValueOnce(error);
    let thrown: unknown = null;
    try {
      await listLibrary();
    } catch (err) {
      thrown = err;
    }
    expect(thrown).toBe(error);
    expect(isAppError(thrown)).toBe(false);
  });

  it('isAppError narrows every documented discriminator', () => {
    for (const code of APP_ERROR_CODES) {
      const value =
        code === 'NotFound'
          ? { code }
          : code === 'UnsupportedMapper'
            ? { code, details: 1 }
            : { code, details: 'msg' };
      expect(isAppError(value)).toBe(true);
    }
    expect(isAppError({ code: 'Unknown' })).toBe(false);
    expect(isAppError(null)).toBe(false);
    expect(isAppError('string')).toBe(false);
    expect(isAppError({})).toBe(false);
  });
});
