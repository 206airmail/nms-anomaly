// A second, deliberately tiny Vite config, used only to run the front end's
// pure logic as a program.
//
// `actions.ts` is the densest piece of decision-making in the app -- it decides
// what the user is told to do and which button appears -- and it had no test at
// all, because the project has no JS test runner and adding one is a dependency
// nobody asked for. Vite is already here, so this bundles the check for the
// server and Node runs it. No browser, no DOM, no test framework.
//
// The main config is not reused because it loads SvelteKit, which wants a
// `src/routes` app and a `$lib` alias resolved through `.svelte-kit`. This needs
// neither: the files under test import each other by relative path.
//
// Paths are relative to the *working directory*, which is Vite's `root` and is
// where `npm run check:actions` starts from. Deliberately not resolved against
// this file, which would need `node:url` and therefore `@types/node`.
import { defineConfig } from "vite";

export default defineConfig({
  build: {
    ssr: "tests/actions.check.ts",
    outDir: ".svelte-kit/checks",
    emptyOutDir: true,
    target: "node20",
    minify: false,
    rollupOptions: {
      output: { entryFileNames: "actions.check.mjs" },
    },
  },
  // Keeps the bundle to the files under test: anything Tauri-shaped would only
  // fail to load outside the app, and nothing being tested touches it.
  ssr: { noExternal: true },
});
