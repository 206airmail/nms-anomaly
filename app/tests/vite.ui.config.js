// A third tiny Vite config, for running the *components* as a program.
//
// `tests/vite.config.js` next door bundles pure logic and needs no compiler.
// This one does: it renders the panes with `svelte/server`, so the Svelte
// plugin has to be here to compile both `.svelte` components and the `.svelte.ts`
// rune modules the stores live in.
//
// Why bother, when `svelte-check` already passes: a type-check does not execute
// anything. It cannot catch a `$derived` that reads a store before it is filled,
// a template that indexes a null, a snippet passed where a component expects a
// list, or a join between two shapes that agree in type and disagree in fact.
// Rendering does, and it needs no browser and no test framework -- the whole
// point of doing it on the server.
//
// The main config is not reused because it loads SvelteKit, which wants routes
// and a `$lib` alias resolved through `.svelte-kit`. This needs neither: the
// check imports the components by relative path.
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte({ compilerOptions: { css: "external" } })],
  build: {
    ssr: "tests/ui.check.ts",
    outDir: ".svelte-kit/checks",
    emptyOutDir: false,
    target: "node20",
    minify: false,
    rollupOptions: {
      output: { entryFileNames: "ui.check.mjs" },
    },
  },
  // Keeps the bundle self-contained so Node can run it straight out of the
  // output folder. The Tauri modules come along too: they are import-safe
  // outside the app -- every call in `engine.ts` is behind an `inTauri()`
  // guard -- and leaving them external would only make this fail to load.
  ssr: { noExternal: true },
});
