// `RomCard` — compact tile rendering a single ROM in the library grid.
//
// Shows the user-visible title, the iNES mapper number, and the size in KB.
// A primary "Play" button triggers `onPlay(rom.id)`; an optional context
// menu (rename / remove) is rendered via `onRename` / `onRemove`.

import type { ReactElement } from 'react';

import type { RomEntry, RomId } from '../ipc/types';

export interface RomCardProps {
  rom: RomEntry;
  /** Invoked when the user clicks the "Play" button. */
  onPlay: (id: RomId) => void;
  /** Optional remove handler — shown as a secondary action when provided. */
  onRemove?: (id: RomId) => void;
  /** Optional rename handler — shown as a secondary action when provided. */
  onRename?: (id: RomId) => void;
  /** Optional last-played timestamp shown beneath the title. */
  lastPlayedAt?: number;
}

const KB = 1024;

function formatSize(bytes: number): string {
  if (bytes < KB) return `${bytes} B`;
  return `${Math.round(bytes / KB)} KB`;
}

function formatLastPlayed(at: number): string {
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleDateString();
}

export function RomCard({
  rom,
  onPlay,
  onRemove,
  onRename,
  lastPlayedAt,
}: RomCardProps): ReactElement {
  return (
    <article
      className="rom-card"
      aria-label={`ROM: ${rom.title}`}
      data-rom-id={rom.id}
    >
      <header className="rom-card__header">
        <h3 className="rom-card__title" title={rom.title}>
          {rom.title}
        </h3>
        {lastPlayedAt !== undefined ? (
          <span className="rom-card__last-played">
            Last played {formatLastPlayed(lastPlayedAt)}
          </span>
        ) : null}
      </header>
      <dl className="rom-card__meta">
        <div className="rom-card__meta-row">
          <dt>Mapper</dt>
          <dd>{rom.mapper}</dd>
        </div>
        <div className="rom-card__meta-row">
          <dt>Size</dt>
          <dd>{formatSize(rom.size_bytes)}</dd>
        </div>
      </dl>
      <footer className="rom-card__actions">
        <button
          type="button"
          className="button button--primary"
          onClick={(): void => onPlay(rom.id)}
        >
          Play
        </button>
        {onRename ? (
          <button
            type="button"
            className="button button--ghost"
            onClick={(): void => onRename(rom.id)}
          >
            Rename
          </button>
        ) : null}
        {onRemove ? (
          <button
            type="button"
            className="button button--danger"
            onClick={(): void => onRemove(rom.id)}
          >
            Remove
          </button>
        ) : null}
      </footer>
    </article>
  );
}
