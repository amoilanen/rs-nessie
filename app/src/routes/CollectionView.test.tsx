// Component tests for the `CollectionView` route.

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { Collection, LibraryFile, RomEntry } from '../ipc/types';

vi.mock('../ipc/client', () => ({
  listLibrary: vi.fn(),
  createCollection: vi.fn(),
  renameCollection: vi.fn(),
  deleteCollection: vi.fn(),
  addRomToCollection: vi.fn(),
  removeRomFromCollection: vi.fn(),
}));

import {
  createCollection,
  deleteCollection,
  listLibrary,
} from '../ipc/client';
import { useLibraryStore } from '../store/libraryStore';
import { useToastStore } from '../store/toastStore';

import { CollectionView } from './CollectionView';

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

const makeCollection = (overrides: Partial<Collection> = {}): Collection => ({
  id: 'col-default',
  name: 'Default',
  rom_ids: [],
  created_at: 1_700_000_000_000,
  ...overrides,
});

function renderView(): ReturnType<typeof render> {
  return render(
    <MemoryRouter initialEntries={['/collections']}>
      <CollectionView />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.mocked(listLibrary).mockReset();
  vi.mocked(createCollection).mockReset();
  vi.mocked(deleteCollection).mockReset();
});

afterEach(() => {
  useLibraryStore.getState().reset();
  useToastStore.getState().clear();
});

describe('CollectionView', () => {
  it('creates a collection and shows it in the sidebar', async () => {
    const lib: LibraryFile = {
      version: 1,
      roms: [makeRom({ id: 'r1', title: 'R1' })],
      collections: [],
    };
    vi.mocked(listLibrary).mockResolvedValueOnce(lib);
    const created = makeCollection({ id: 'c-new', name: 'Heroes' });
    vi.mocked(createCollection).mockResolvedValueOnce(created);

    renderView();

    // Wait for the initial library load to settle.
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText('New collection…'),
      ).toBeInTheDocument();
    });

    const input = screen.getByLabelText('New collection name');
    fireEvent.change(input, { target: { value: 'Heroes' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create' }));

    await waitFor(() => {
      expect(createCollection).toHaveBeenCalledWith('Heroes');
    });
    await waitFor(() => {
      // The new collection appears in the sidebar; the auto-select also
      // promotes it into the main pane header — both should be present.
      expect(screen.getAllByText('Heroes').length).toBeGreaterThan(0);
    });
  });

  it('shows the confirm dialog before deleting a collection', async () => {
    const collection = makeCollection({ id: 'c1', name: 'Favourites' });
    const lib: LibraryFile = {
      version: 1,
      roms: [],
      collections: [collection],
    };
    vi.mocked(listLibrary).mockResolvedValueOnce(lib);
    vi.mocked(deleteCollection).mockResolvedValueOnce(undefined);

    renderView();

    await waitFor(() => {
      expect(screen.getByText('Favourites')).toBeInTheDocument();
    });

    fireEvent.click(
      screen.getByRole('button', { name: 'Delete Favourites' }),
    );

    // Dialog appears.
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(screen.getByText('Delete collection')).toBeInTheDocument();
    // Delete is NOT called until the user confirms.
    expect(deleteCollection).not.toHaveBeenCalled();

    // Cancel keeps the collection.
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(deleteCollection).not.toHaveBeenCalled();

    // Re-open and confirm.
    fireEvent.click(
      screen.getByRole('button', { name: 'Delete Favourites' }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Delete' }));

    await waitFor(() => {
      expect(deleteCollection).toHaveBeenCalledWith('c1');
    });
  });
});
