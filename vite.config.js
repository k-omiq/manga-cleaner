import { configDefaults, defineConfig } from 'vitest/config'
import { svelte } from '@sveltejs/vite-plugin-svelte'

/**
 * How a test asks for a browser: **name it `*.dom.test.js`**.
 *
 * The suite is two vitest projects rather than one. Almost every test in
 * `src/lib` is arithmetic over plain objects, and a DOM per file costs a
 * second of start-up for nothing - so `node` is the
 * default and stays it. The handful of files that mount a component need two
 * things a node run cannot give them: a document, and Svelte resolved through
 * its **browser** export rather than its server one, because vitest resolves
 * dependencies with node conditions and hands back the SSR build, where
 * `mount()` throws `lifecycle_function_unavailable`.
 *
 * The second half is why the file-level `// @vitest-environment jsdom` pragma
 * is not enough on its own: vitest honours it for the environment and has no
 * equivalent for `resolve.conditions`, which is a property of the project a
 * file belongs to. The suffix is what puts a file in the project that has
 * both, and a glob is what a project can select on.
 */
const DOM_SUFFIX = 'src/**/*.dom.test.js'

export default defineConfig({
  plugins: [svelte()],
  test: {
    // Two projects, one command: `npm test` runs both and reports one total.
    projects: [
      {
        // `extends: true` takes the plugins above, so a node test may still
        // import a `.svelte` module script - `SettingsDialog.test.js` does.
        extends: true,
        test: {
          name: 'node',
          environment: 'node',
          include: ['src/**/*.test.js'],
          exclude: [...configDefaults.exclude, DOM_SUFFIX],
        },
      },
      {
        extends: true,
        // Scoped to the project that mounts things. Nothing depends on
        // Svelte's server build today; the day something does, it is a node
        // test and this no longer reaches it.
        resolve: { conditions: ['browser'] },
        test: {
          name: 'jsdom',
          environment: 'jsdom',
          include: [DOM_SUFFIX],
        },
      },
    ],
  },
})
