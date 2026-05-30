// Lightweight global toast store.
//
// Components push transient notifications (success or error) by calling
// `useToastStore.getState().push({ kind, message })`. The single mounted
// `<Toast />` portal renders them and dismisses them on a timer.

import { create } from 'zustand';

export type ToastKind = 'info' | 'success' | 'error';

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

export interface ToastState {
  toasts: Toast[];
  push: (toast: Omit<Toast, 'id'>) => number;
  dismiss: (id: number) => void;
  clear: () => void;
}

let nextId = 1;

export const useToastStore = create<ToastState>((set) => ({
  toasts: [],

  push: (toast) => {
    const id = nextId++;
    set((state) => ({ toasts: [...state.toasts, { ...toast, id }] }));
    return id;
  },

  dismiss: (id) =>
    set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) })),

  clear: () => set({ toasts: [] }),
}));

/** Convenience helper used by view-level effects. */
export function pushErrorToast(message: string): number {
  return useToastStore.getState().push({ kind: 'error', message });
}

/** Convenience helper for success notifications. */
export function pushSuccessToast(message: string): number {
  return useToastStore.getState().push({ kind: 'success', message });
}
