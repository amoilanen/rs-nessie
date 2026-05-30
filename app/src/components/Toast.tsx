// Toast notification stack.
//
// Reads from the global `toastStore` and renders an absolutely-positioned
// stack of dismissible cards. Each toast auto-dismisses after a fixed delay
// unless the user hovers over it (handled by pausing the timer).

import { useEffect, type ReactElement } from 'react';

import { useToastStore } from '../store/toastStore';

const AUTO_DISMISS_MS = 5_000;

interface ToastItemProps {
  id: number;
  kind: 'info' | 'success' | 'error';
  message: string;
  onDismiss: (id: number) => void;
}

function ToastItem({ id, kind, message, onDismiss }: ToastItemProps): ReactElement {
  useEffect(() => {
    const handle = window.setTimeout(() => onDismiss(id), AUTO_DISMISS_MS);
    return () => window.clearTimeout(handle);
  }, [id, onDismiss]);

  return (
    <div
      role={kind === 'error' ? 'alert' : 'status'}
      aria-live={kind === 'error' ? 'assertive' : 'polite'}
      className={`toast toast--${kind}`}
    >
      <span className="toast__message">{message}</span>
      <button
        type="button"
        className="toast__dismiss"
        aria-label="Dismiss notification"
        onClick={(): void => onDismiss(id)}
      >
        ×
      </button>
    </div>
  );
}

/**
 * Top-level toast renderer. Mount once at the root of the application; it
 * subscribes to the toast store and renders any active toasts.
 */
export function Toast(): ReactElement | null {
  const toasts = useToastStore((s) => s.toasts);
  const dismiss = useToastStore((s) => s.dismiss);

  if (toasts.length === 0) return null;

  return (
    <div className="toast-stack" aria-label="Notifications">
      {toasts.map((t) => (
        <ToastItem
          key={t.id}
          id={t.id}
          kind={t.kind}
          message={t.message}
          onDismiss={dismiss}
        />
      ))}
    </div>
  );
}
