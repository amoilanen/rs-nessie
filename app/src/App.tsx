import type { ReactElement } from 'react';
import { HashRouter, NavLink, Navigate, Outlet, Route, Routes } from 'react-router-dom';

import { Toast } from './components/Toast';
import { LibraryView } from './routes/LibraryView';
import { CollectionView } from './routes/CollectionView';
import { GameView } from './routes/GameView';
import { SettingsView } from './routes/SettingsView';

// Top-level application shell.
//
// A persistent left-hand sidebar exposes the four primary routes; the right-
// hand pane renders the active route via `<Outlet />`. The Game tab is
// rendered as disabled until an emulation session is active — that wiring is
// completed in the "Game view" step.

interface NavItem {
  to: string;
  label: string;
  /** When `false` the link is rendered disabled. */
  enabled: boolean;
}

function NavSidebar({ items }: { items: NavItem[] }): ReactElement {
  return (
    <aside className="app-sidebar" aria-label="Primary navigation">
      <div className="app-sidebar__brand">rs-nessie</div>
      <nav className="app-sidebar__nav">
        {items.map((item) => {
          const className = ({ isActive }: { isActive: boolean }): string => {
            const classes = ['app-sidebar__link'];
            if (isActive) classes.push('is-active');
            if (!item.enabled) classes.push('is-disabled');
            return classes.join(' ');
          };
          return (
            <NavLink
              key={item.to}
              to={item.to}
              className={className}
              aria-disabled={!item.enabled || undefined}
              end={item.to === '/'}
            >
              {item.label}
            </NavLink>
          );
        })}
      </nav>
    </aside>
  );
}

function AppLayout(): ReactElement {
  // The Game tab is disabled until a session is active. Session presence
  // wiring lives in a later step; for now we render it disabled.
  const items: NavItem[] = [
    { to: '/library', label: 'Library', enabled: true },
    { to: '/collections', label: 'Collections', enabled: true },
    { to: '/game', label: 'Game', enabled: false },
    { to: '/settings', label: 'Settings', enabled: true },
  ];

  return (
    <div className="app-shell">
      <NavSidebar items={items} />
      <main className="app-main">
        <Outlet />
      </main>
      <Toast />
    </div>
  );
}

export function App(): ReactElement {
  return (
    <HashRouter>
      <Routes>
        <Route element={<AppLayout />}>
          <Route index element={<Navigate to="/library" replace />} />
          <Route path="/library" element={<LibraryView />} />
          <Route path="/collections" element={<CollectionView />} />
          <Route path="/game" element={<GameView />} />
          <Route path="/settings" element={<SettingsView />} />
          <Route path="*" element={<Navigate to="/library" replace />} />
        </Route>
      </Routes>
    </HashRouter>
  );
}
