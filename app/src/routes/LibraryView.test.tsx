// Component tests for the `LibraryView` route.
//
// The IPC client and the toast store are mocked. The library store is reset
// between tests via its `reset` action.

import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { LibraryFile, RomEntry } from '../ipc/types';

vi.mock('../ipc/client', () => ({
  listLibrary: vi.fn(),
  importRomFromDialog: vi.fn(),
  removeRomFromLibrary: vi.fn(),
  renameRom: vi.fn(),
}));

import { listLibrary } from '../ipc/client';
import { useLibraryStore } from '../store/libraryStore';
import { useToastStore } from '../store/toastStore';

import { LibraryView } from './LibraryView';

const makeRom = (overrides: Partial<RomEntry> = {}): RomEntry => ({
  id: 'rom-default',
  title: 'Default',
  path: '/tmp/default.nes',
  sha1: 'a'.repeat(40),
  mapper: 0,
  size_bytes: 24_576,
  imported_at: 1_700_000_000_000,
  ...overrides,
});

function renderView(): ReturnType<typeof render> {
  return render(
    <MemoryRouter initialEntries={['/library']}>
      <LibraryView />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.mocked(listLibrary).mockReset();
});

afterEach(() => {
  useLibraryStore.getState().reset();
  useToastStore.getState().clear();
});

describe('LibraryView', () => {
  it('shows the empty-state CTA when the library has no ROMs', async () => {
    const empty: LibraryFile = { version: 1, roms: [], collections: [] };
    vi.mocked(listLibrary).mockResolvedValueOnce(empty);

    renderView();

    await waitFor(() => {
      expect(screen.getByText('Your library is empty')).toBeInTheDocument();
    });
    expect(
      screen.getByRole('button', { name: 'Import your first ROM' }),
    ).toBeInTheDocument();
  });

  it('lists ROMs in most-recently-imported order when nothing has been played', async () => {
    const a = makeRom({ id: 'a', title: 'Alpha', imported_at: 1 });
    const b = makeRom({ id: 'b', title: 'Bravo', imported_at: 3 });
    const c = makeRom({ id: 'c', title: 'Charlie', imported_at: 2 });
    const populated: LibraryFile = {
      version: 1,
      roms: [a, b, c],
      collections: [],
    };
    vi.mocked(listLibrary).mockResolvedValueOnce(populated);

    renderView();

    await waitFor(() => {
      expect(screen.getByText('Alpha')).toBeInTheDocument();
    });
    const titles = screen
      .getAllByRole('listitem')
      .map(
        (item) =>
          item.querySelector('.rom-card__title')?.textContent ?? '',
      );
    // Sort order: by imported_at desc → Bravo (3), Charlie (2), Alpha (1).
    expect(titles).toEqual(['Bravo', 'Charlie', 'Alpha']);
  });

  it('shows an error toast when listLibrary rejects', async () => {
    vi.mocked(listLibrary).mockRejectedValueOnce({
      code: 'LibraryCorrupted',
      details: 'bad json',
    });

    renderView();

    await waitFor(() => {
      const toasts = useToastStore.getState().toasts;
      expect(toasts).toHaveLength(1);
      expect(toasts[0]?.kind).toBe('error');
      expect(toasts[0]?.message).toContain('corrupted');
    });
  });
});
