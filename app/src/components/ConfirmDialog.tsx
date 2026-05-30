// Modal confirmation dialog.
//
// Used for destructive actions (e.g. deleting a collection, removing a ROM).
// Rendered as a controlled component — visibility is driven by the `open`
// prop and the parent owns the `onConfirm` / `onCancel` callbacks.

import { useEffect, type ReactElement } from 'react';

export interface ConfirmDialogProps {
  /** Whether the modal is visible. */
  open: boolean;
  /** Dialog title. */
  title: string;
  /** Optional body / explanation text. */
  description?: string;
  /** Label of the confirm button (defaults to "Confirm"). */
  confirmLabel?: string;
  /** Label of the cancel button (defaults to "Cancel"). */
  cancelLabel?: string;
  /** Treat the confirm action as destructive (red button). */
  destructive?: boolean;
  /** Invoked when the user accepts. */
  onConfirm: () => void;
  /** Invoked when the user cancels or presses Escape. */
  onCancel: () => void;
}

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel = 'Confirm',
  cancelLabel = 'Cancel',
  destructive = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps): ReactElement | null {
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') onCancel();
    };
    window.addEventListener('keydown', handler);
    return (): void => window.removeEventListener('keydown', handler);
  }, [open, onCancel]);

  if (!open) return null;

  return (
    <div
      className="modal-backdrop"
      role="dialog"
      aria-modal="true"
      aria-labelledby="confirm-dialog-title"
      onClick={onCancel}
    >
      <div
        className="modal"
        onClick={(e): void => e.stopPropagation()}
      >
        <h2 id="confirm-dialog-title" className="modal__title">
          {title}
        </h2>
        {description ? (
          <p className="modal__description">{description}</p>
        ) : null}
        <div className="modal__actions">
          <button
            type="button"
            className="button button--ghost"
            onClick={onCancel}
          >
            {cancelLabel}
          </button>
          <button
            type="button"
            className={`button ${destructive ? 'button--danger' : 'button--primary'}`}
            onClick={onConfirm}
            autoFocus
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
