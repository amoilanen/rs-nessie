// Import-ROM button.
//
// Triggers the native file-open dialog via the `importRomFromDialog` IPC
// wrapper, updates the library store with the resulting entry, and surfaces
// any [`AppError`] returned by the backend as a toast.

import { useCallback, useState, type ReactElement } from 'react';

import { importRomFromDialog } from '../ipc/client';
import { formatAppError } from '../ipc/format';
import type { RomEntry } from '../ipc/types';
import { useLibraryStore } from '../store/libraryStore';
import { pushErrorToast, pushSuccessToast } from '../store/toastStore';

export interface ImportButtonProps {
  /** Optional label override (defaults to "Import ROM"). */
  label?: string;
  /** Optional callback invoked after a successful import. */
  onImported?: (rom: RomEntry) => void;
}

export function ImportButton({
  label = 'Import ROM',
  onImported,
}: ImportButtonProps): ReactElement {
  const [busy, setBusy] = useState(false);
  const upsertRom = useLibraryStore((s) => s.upsertRom);

  const handleClick = useCallback(async (): Promise<void> => {
    if (busy) return;
    setBusy(true);
    try {
      const rom = await importRomFromDialog();
      if (rom) {
        upsertRom(rom);
        pushSuccessToast(`Imported "${rom.title}".`);
        onImported?.(rom);
      }
    } catch (err) {
      pushErrorToast(formatAppError(err));
    } finally {
      setBusy(false);
    }
  }, [busy, upsertRom, onImported]);

  return (
    <button
      type="button"
      className="button button--primary"
      onClick={(): void => {
        void handleClick();
      }}
      disabled={busy}
    >
      {busy ? 'Importing…' : label}
    </button>
  );
}
