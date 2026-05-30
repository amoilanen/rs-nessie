// Collections route.
//
// Two-pane layout: a sidebar `CollectionList` for creating / renaming /
// deleting collections, and a main pane that lists the ROMs in the selected
// collection plus a modal picker for adding ROMs from the library to it.

import { useCallback, useEffect, useMemo, useState, type ReactElement } from 'react';
import { useNavigate } from 'react-router-dom';

import { CollectionList } from '../components/CollectionList';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { RomCard } from '../components/RomCard';
import {
  addRomToCollection,
  createCollection,
  deleteCollection,
  listLibrary,
  removeRomFromCollection,
  renameCollection,
} from '../ipc/client';
import { formatAppError } from '../ipc/format';
import type { CollectionId, RomId } from '../ipc/types';
import { useLibraryStore } from '../store/libraryStore';
import { pushErrorToast } from '../store/toastStore';

export function CollectionView(): ReactElement {
  const navigate = useNavigate();
  const setLibrary = useLibraryStore((s) => s.setLibrary);
  const setLoading = useLibraryStore((s) => s.setLoading);
  const setError = useLibraryStore((s) => s.setError);
  const upsertCollection = useLibraryStore((s) => s.upsertCollection);
  const removeCollection = useLibraryStore((s) => s.removeCollection);
  const markPlayed = useLibraryStore((s) => s.markPlayed);

  const library = useLibraryStore((s) => s.library);
  const allCollections = useMemo(
    () => library?.collections ?? [],
    [library],
  );
  const allRoms = useMemo(() => library?.roms ?? [], [library]);

  const [selectedId, setSelectedId] = useState<CollectionId | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<CollectionId | null>(
    null,
  );
  const [pickerOpen, setPickerOpen] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    listLibrary()
      .then((lib) => {
        if (cancelled) return;
        setLibrary(lib);
      })
      .catch((err) => {
        if (cancelled) return;
        const message = formatAppError(err);
        setError(message);
        pushErrorToast(message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return (): void => {
      cancelled = true;
    };
  }, [setLibrary, setLoading, setError]);

  // Auto-select the first collection once available.
  useEffect(() => {
    if (selectedId !== null) return;
    const first = allCollections[0];
    if (first) setSelectedId(first.id);
  }, [allCollections, selectedId]);

  const selectedCollection = useMemo(
    () =>
      selectedId
        ? (allCollections.find((c) => c.id === selectedId) ?? null)
        : null,
    [allCollections, selectedId],
  );
  const collectionRoms = useMemo(() => {
    if (!selectedCollection) return [];
    const ids = new Set(selectedCollection.rom_ids);
    return allRoms.filter((r) => ids.has(r.id));
  }, [allRoms, selectedCollection]);

  const availableRoms = useMemo(() => {
    if (!selectedCollection) return [];
    const member = new Set(selectedCollection.rom_ids);
    return allRoms.filter((r) => !member.has(r.id));
  }, [allRoms, selectedCollection]);

  const handleCreate = useCallback(
    async (name: string): Promise<void> => {
      try {
        const created = await createCollection(name);
        upsertCollection(created);
        setSelectedId(created.id);
      } catch (err) {
        pushErrorToast(formatAppError(err));
      }
    },
    [upsertCollection],
  );

  const handleRename = useCallback(
    async (id: CollectionId, name: string): Promise<void> => {
      try {
        const updated = await renameCollection(id, name);
        upsertCollection(updated);
      } catch (err) {
        pushErrorToast(formatAppError(err));
      }
    },
    [upsertCollection],
  );

  const handleConfirmDelete = useCallback(async (): Promise<void> => {
    const id = pendingDeleteId;
    if (!id) return;
    setPendingDeleteId(null);
    try {
      await deleteCollection(id);
      removeCollection(id);
      if (selectedId === id) setSelectedId(null);
    } catch (err) {
      pushErrorToast(formatAppError(err));
    }
  }, [pendingDeleteId, removeCollection, selectedId]);

  const handleAddRom = useCallback(
    async (romId: RomId): Promise<void> => {
      if (!selectedCollection) return;
      try {
        await addRomToCollection(selectedCollection.id, romId);
        upsertCollection({
          ...selectedCollection,
          rom_ids: [...selectedCollection.rom_ids, romId],
        });
      } catch (err) {
        pushErrorToast(formatAppError(err));
      }
    },
    [selectedCollection, upsertCollection],
  );

  const handleRemoveRom = useCallback(
    async (romId: RomId): Promise<void> => {
      if (!selectedCollection) return;
      try {
        await removeRomFromCollection(selectedCollection.id, romId);
        upsertCollection({
          ...selectedCollection,
          rom_ids: selectedCollection.rom_ids.filter((id) => id !== romId),
        });
      } catch (err) {
        pushErrorToast(formatAppError(err));
      }
    },
    [selectedCollection, upsertCollection],
  );

  const handlePlay = useCallback(
    (id: RomId): void => {
      markPlayed(id);
      navigate(`/game?rom=${encodeURIComponent(id)}`);
    },
    [markPlayed, navigate],
  );

  return (
    <section className="collection-view" aria-labelledby="collections-heading">
      <h1 id="collections-heading" className="visually-hidden">
        Collections
      </h1>
      <CollectionList
        collections={allCollections}
        selectedId={selectedId}
        onSelect={setSelectedId}
        onCreate={(name): void => {
          void handleCreate(name);
        }}
        onRename={(id, name): void => {
          void handleRename(id, name);
        }}
        onRequestDelete={(id): void => setPendingDeleteId(id)}
      />

      <div className="collection-view__main">
        {selectedCollection ? (
          <>
            <header className="collection-view__header">
              <h2>{selectedCollection.name}</h2>
              <div className="collection-view__controls">
                <button
                  type="button"
                  className="button button--primary"
                  onClick={(): void => setPickerOpen(true)}
                  disabled={availableRoms.length === 0}
                >
                  Add ROM
                </button>
              </div>
            </header>
            {collectionRoms.length === 0 ? (
              <p className="collection-view__empty">
                No ROMs in this collection yet. Click <strong>Add ROM</strong>
                {' '}to include one from your library.
              </p>
            ) : (
              <div className="library-view__grid" role="list">
                {collectionRoms.map((rom) => (
                  <div role="listitem" key={rom.id}>
                    <RomCard
                      rom={rom}
                      onPlay={handlePlay}
                      onRemove={(id): void => {
                        void handleRemoveRom(id);
                      }}
                    />
                  </div>
                ))}
              </div>
            )}
          </>
        ) : (
          <div className="collection-view__placeholder" role="status">
            <p>Select or create a collection to get started.</p>
          </div>
        )}
      </div>

      <ConfirmDialog
        open={pendingDeleteId !== null}
        title="Delete collection"
        description="The ROMs in the collection are not deleted — only the collection itself."
        confirmLabel="Delete"
        destructive
        onCancel={(): void => setPendingDeleteId(null)}
        onConfirm={(): void => {
          void handleConfirmDelete();
        }}
      />

      {pickerOpen && selectedCollection ? (
        <div
          className="modal-backdrop"
          role="dialog"
          aria-modal="true"
          aria-labelledby="add-rom-dialog-title"
          onClick={(): void => setPickerOpen(false)}
        >
          <div
            className="modal modal--wide"
            onClick={(e): void => e.stopPropagation()}
          >
            <h2 id="add-rom-dialog-title" className="modal__title">
              Add ROM to {selectedCollection.name}
            </h2>
            {availableRoms.length === 0 ? (
              <p>Every ROM in your library is already in this collection.</p>
            ) : (
              <ul className="modal__list" role="list">
                {availableRoms.map((rom) => (
                  <li key={rom.id} className="modal__list-item">
                    <span className="modal__list-title">{rom.title}</span>
                    <button
                      type="button"
                      className="button button--primary button--small"
                      onClick={(): void => {
                        void handleAddRom(rom.id);
                      }}
                    >
                      Add
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <div className="modal__actions">
              <button
                type="button"
                className="button button--ghost"
                onClick={(): void => setPickerOpen(false)}
              >
                Close
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </section>
  );
}
