// The site is its own Vercel project and its own test root. Without this,
// vitest walks up and picks up the app's root vite.config.ts (React plugin,
// browser environment, a web/ include glob) and collects zero suites here.
//
// It lives under api/_tests/ rather than at site/ because Vercel turns every
// non-underscore file in api/ into a function, and because this workstream owns
// api/** outright. package.json points at it.

import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    root: import.meta.dirname + '/../..',
    include: ['api/_tests/**/*.test.ts'],
    environment: 'node',
    globals: false,
    clearMocks: true,
    restoreMocks: true,
  },
})
