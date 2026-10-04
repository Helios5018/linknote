import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';

await build({
  stdin: {
    contents: `import mermaid from 'mermaid';
      import zenuml from '@mermaid-js/mermaid-zenuml';
      mermaid.registerExternalDiagrams([zenuml]);
      window.mermaid = mermaid;`,
    resolveDir: fileURLToPath(new URL('.', import.meta.url)),
  },
  outfile: fileURLToPath(new URL('bundle.js', import.meta.url)),
  bundle: true,
  minify: true,
  format: 'iife',
  platform: 'browser',
  logLevel: 'warning',
});
