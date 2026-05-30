// `CollectionList` — sidebar of user-defined collections.
//
// Renders the existing collections with selection state and provides
// affordances for creating, renaming, and deleting collections. All
// destructive actions are routed through the parent so a confirmation dialog
// can be inserted in between.

import { useState, type FormEvent, type ReactElement } from 'react';

import type { Collection, CollectionId } from '../ipc/types';

export interface CollectionListProps {
  collections: Collection[];
  /** Currently-selected collection id, or `null`. */
  selectedId: CollectionId | null;
  /** Invoked when the user picks a different collection. */
  onSelect: (id: CollectionId) => void;
  /** Invoked when the user submits the "new collection" form. */
  onCreate: (name: string) => void;
  /** Invoked when the user requests to rename a collection in-place. */
  onRename: (id: CollectionId, name: string) => void;
  /** Invoked when the user requests deletion. The parent should confirm. */
  onRequestDelete: (id: CollectionId) => void;
}

export function CollectionList({
  collections,
  selectedId,
  onSelect,
  onCreate,
  onRename,
  onRequestDelete,
}: CollectionListProps): ReactElement {
  const [draftName, setDraftName] = useState('');
  const [editingId, setEditingId] = useState<CollectionId | null>(null);
  const [editingValue, setEditingValue] = useState('');

  const handleCreateSubmit = (e: FormEvent<HTMLFormElement>): void => {
    e.preventDefault();
    const trimmed = draftName.trim();
    if (!trimmed) return;
    onCreate(trimmed);
    setDraftName('');
  };

  const beginEditing = (collection: Collection): void => {
    setEditingId(collection.id);
    setEditingValue(collection.name);
  };

  const commitEditing = (id: CollectionId): void => {
    const trimmed = editingValue.trim();
    if (trimmed) onRename(id, trimmed);
    setEditingId(null);
    setEditingValue('');
  };

  return (
    <aside className="collection-list" aria-label="Collections">
      <header className="collection-list__header">
        <h2 className="collection-list__title">Collections</h2>
      </header>
      <form className="collection-list__new" onSubmit={handleCreateSubmit}>
        <input
          type="text"
          aria-label="New collection name"
          placeholder="New collection…"
          value={draftName}
          onChange={(e): void => setDraftName(e.currentTarget.value)}
        />
        <button
          type="submit"
          className="button button--primary"
          disabled={draftName.trim().length === 0}
        >
          Create
        </button>
      </form>
      <ul className="collection-list__items" role="list">
        {collections.length === 0 ? (
          <li className="collection-list__empty">No collections yet.</li>
        ) : (
          collections.map((collection) => {
            const isSelected = collection.id === selectedId;
            const isEditing = collection.id === editingId;
            return (
              <li
                key={collection.id}
                className={`collection-list__item ${isSelected ? 'is-selected' : ''}`}
              >
                {isEditing ? (
                  <form
                    onSubmit={(e): void => {
                      e.preventDefault();
                      commitEditing(collection.id);
                    }}
                    className="collection-list__edit"
                  >
                    <input
                      type="text"
                      aria-label={`Rename collection ${collection.name}`}
                      value={editingValue}
                      onChange={(e): void => setEditingValue(e.currentTarget.value)}
                      autoFocus
                      onKeyDown={(e): void => {
                        if (e.key === 'Escape') {
                          setEditingId(null);
                          setEditingValue('');
                        }
                      }}
                    />
                    <button type="submit" className="button button--ghost">
                      Save
                    </button>
                  </form>
                ) : (
                  <>
                    <button
                      type="button"
                      className="collection-list__select"
                      onClick={(): void => onSelect(collection.id)}
                      aria-pressed={isSelected}
                    >
                      <span className="collection-list__name">
                        {collection.name}
                      </span>
                      <span className="collection-list__count">
                        {collection.rom_ids.length}
                      </span>
                    </button>
                    <div className="collection-list__actions">
                      <button
                        type="button"
                        className="button button--ghost button--small"
                        aria-label={`Rename ${collection.name}`}
                        onClick={(): void => beginEditing(collection)}
                      >
                        Rename
                      </button>
                      <button
                        type="button"
                        className="button button--danger button--small"
                        aria-label={`Delete ${collection.name}`}
                        onClick={(): void => onRequestDelete(collection.id)}
                      >
                        Delete
                      </button>
                    </div>
                  </>
                )}
              </li>
            );
          })
        )}
      </ul>
    </aside>
  );
}
