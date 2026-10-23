// Regenerates the CLI and MCP reference pages from the Rust code.
// Run from docs-site/: npm run reference
import { execFileSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';

for (const page of ['cli', 'mcp']) {
  const markdown = execFileSync(
    'cargo',
    ['run', '--quiet', '-p', 'convert-cli', '--', 'reference', page],
    { cwd: new URL('../..', import.meta.url), encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] },
  );
  const path = new URL(`../src/content/docs/reference/${page}.mdx`, import.meta.url);
  writeFileSync(path, markdown);
  console.log(`Wrote src/content/docs/reference/${page}.mdx`);
}
