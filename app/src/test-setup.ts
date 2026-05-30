// Vitest setup file. Extends the `expect` global with the
// `@testing-library/jest-dom` matchers (`toBeInTheDocument`, `toHaveTextContent`,
// …) and registers an `afterEach` cleanup so the DOM is unmounted between tests.

import '@testing-library/jest-dom/vitest';
import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';

afterEach(() => {
  cleanup();
});
