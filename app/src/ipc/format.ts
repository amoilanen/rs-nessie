// Helpers for translating IPC-layer errors into user-facing strings.
//
// The Rust host returns structured [`AppError`] envelopes (see spec §5.2 and
// `./app/src-tauri/src/error.rs`). Several of the discriminator arms — most
// notably `InvalidRom`, `UnsupportedMapper`, and `RomMissing` — must surface
// as friendly, localizable messages per FR-8 and FR-14.

import { isAppError } from './types';

/**
 * Render an [`AppError`] (or any caught value) as a short, human-readable
 * message suitable for a toast / inline error banner.
 *
 * Unknown values fall back to a generic "Unexpected error" string so the UI
 * never renders `[object Object]`.
 */
export function formatAppError(error: unknown): string {
  if (isAppError(error)) {
    switch (error.code) {
      case 'InvalidRom':
        return `That file is not a valid NES ROM (${error.details}).`;
      case 'UnsupportedMapper':
        return `This ROM uses mapper ${error.details}, which rs-nessie does not yet support.`;
      case 'RomMissing':
        return `The ROM file could not be found on disk: ${error.details}.`;
      case 'LibraryCorrupted':
        return `Your ROM library file is corrupted: ${error.details}.`;
      case 'NotFound':
        return 'The requested item could not be found.';
      case 'Io':
        return `Filesystem error: ${error.details}.`;
    }
  }
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'Unexpected error.';
}
