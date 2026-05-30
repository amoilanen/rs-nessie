// Library route.
//
// Renders the user's ROM library as a card grid with import / search /
// remove affordances. ROMs are sorted by `lastPlayedAt` first (descending),
// then by `imported_at` (descending) so freshly-imported ROMs surface at the
// top when nothing has been played yet (FR-6, FR-9).

import { useCallback, useEffect, useMemo, useState, type ReactElement } from 'react';
import { useNavigate } from 'react-router-dom';

import { ConfirmDialog } from '../components/ConfirmDialog';
import { ImportButton } from '../components/ImportButton';
import { RomCard } from '../components/RomCard';
import { listLibrary, removeRomFromLibrary, renameRom } from '../ipc/client';
import { formatAppError } from '../ipc/format';
import type { RomId } from '../ipc/types';
import { useLibraryStore } from '../store/libraryStore';
import { pushErrorToast } from '../store/toastStore';

export function LibraryView(): ReactElement {
  const navigate = useNavigate();
  const setLibrary = useLibraryStore((s) => s.setLibrary);
  const setLoading = useLibraryStore((s) => s.setLoading);
  const setError = useLibraryStore((s) => s.setError);
  const removeRom = useLibraryStore((s) => s.removeRom);
  const upsertRom = useLibraryStore((s) => s.upsertRom);
  const markPlayed = useLibraryStore((s) => s.markPlayed);
  const lastPlayedAt = useLibraryStore((s) => s.lastPlayedAt);
  const library = useLibraryStore((s) => s.library);

  const sorted = useMemo(() => {
    const roms = library?.roms ?? [];
    return [...roms].sort((a, b) => {
      const pa = lastPlayedAt[a.id] ?? 0;
      const pb = lastPlayedAt[b.id] ?? 0;
      if (pa !== pb) return pb - pa;
      return b.imported_at - a.imported_at;
    });
  }, [library, lastPlayedAt]);

  const [query, setQuery] = useState('');
  const [pendingDeleteId, setPendingDeleteId] = useState<RomId | null>(null);

  // Initial fetch of the library snapshot. Runs once on mount.
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

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return sorted;
    return sorted.filter((r) => r.title.toLowerCase().includes(needle));
  }, [query, sorted]);

  const handlePlay = useCallback(
    (id: RomId): void => {
      markPlayed(id);
      navigate(`/game?rom=${encodeURIComponent(id)}`);
    },
    [markPlayed, navigate],
  );

  const handleRemove = useCallback((id: RomId): void => {
    setPendingDeleteId(id);
  }, []);

  const handleConfirmDelete = useCallback(async (): Promise<void> => {
    const id = pendingDeleteId;
    if (!id) return;
    setPendingDeleteId(null);
    try {
      await removeRomFromLibrary(id);
      removeRom(id);
    } catch (err) {
      pushErrorToast(formatAppError(err));
    }
  }, [pendingDeleteId, removeRom]);

  const handleRename = useCallback(
    async (id: RomId): Promise<void> => {
      const current = library?.roms.find((r) => r.id === id);
      const next = window.prompt('New title', current?.title ?? '');
      if (next == null) return;
      const trimmed = next.trim();
      if (!trimmed || trimmed === current?.title) return;
      try {
        const updated = await renameRom(id, trimmed);
        upsertRom(updated);
      } catch (err) {
        pushErrorToast(formatAppError(err));
      }
    },
    [library, upsertRom],
  );

  const isEmpty = (library?.roms.length ?? 0) === 0;

  return (
    <section className="library-view" aria-labelledby="library-heading">
      <header className="library-view__header">
        <h1 id="library-heading">Library</h1>
        <div className="library-view__controls">
          <input
            type="search"
            className="library-view__search"
            aria-label="Search ROMs"
            placeholder="Search ROMs…"
            value={query}
            onChange={(e): void => setQuery(e.currentTarget.value)}
          />
          <ImportButton />
        </div>
      </header>

      {isEmpty ? (
        <div className="library-view__empty" role="status">
          <h2>Your library is empty</h2>
          <p>Import your first ROM to start playing.</p>
          <ImportButton label="Import your first ROM" />
        </div>
      ) : filtered.length === 0 ? (
        <div className="library-view__empty" role="status">
          <p>No ROMs match &ldquo;{query}&rdquo;.</p>
        </div>
      ) : (
        <div className="library-view__grid" role="list">
          {filtered.map((rom) => {
            const cardProps = {
              rom,
              onPlay: handlePlay,
              onRemove: handleRemove,
              onRename: (id: RomId): void => {
                void handleRename(id);
              },
              ...(lastPlayedAt[rom.id] !== undefined
                ? { lastPlayedAt: lastPlayedAt[rom.id] as number }
                : {}),
            } as const;
            return (
              <div role="listitem" key={rom.id}>
                <RomCard {...cardProps} />
              </div>
            );
          })}
        </div>
      )}

      <ConfirmDialog
        open={pendingDeleteId !== null}
        title="Remove ROM"
        description="This will remove the ROM from your library and from every collection it belongs to. The ROM file itself is left untouched on disk."
        confirmLabel="Remove"
        destructive
        onCancel={(): void => setPendingDeleteId(null)}
        onConfirm={(): void => {
          void handleConfirmDelete();
        }}
      />
    </section>
  );
}
