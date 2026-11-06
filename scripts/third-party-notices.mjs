// Writes THIRD-PARTY-NOTICES.txt: licenses of the Rust crates (via cargo-about)
// followed by those of the production npm packages (via pnpm).
// Usage: node scripts/third-party-notices.mjs [output]
// Requires `cargo install cargo-about --locked --features cli` and a prior `pnpm install`.
import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const out = process.argv[2] ?? 'THIRD-PARTY-NOTICES.txt';
const run = (cmd, args) =>
  execFileSync(cmd, args, {
    encoding: 'utf8',
    maxBuffer: 256 * 1024 * 1024,
    shell: process.platform === 'win32',
  });

const rust = run('cargo', ['about', 'generate', 'about.hbs']);

// devDependencies whose code is compiled into the shipped frontend bundle (the Svelte runtime).
const bundledDev = new Set(['svelte', 'clsx', 'esm-env']);
const bySpdx = {};
for (const [spdx, pkgs] of Object.entries(JSON.parse(run('pnpm', ['licenses', 'list', '--json'])))) {
  const keep = pkgs.filter((p) => bundledDev.has(p.name));
  if (keep.length) bySpdx[spdx] = keep;
}
for (const [spdx, pkgs] of Object.entries(JSON.parse(run('pnpm', ['licenses', 'list', '--prod', '--json'])))) {
  bySpdx[spdx] = [...(bySpdx[spdx] ?? []), ...pkgs.filter((p) => !bundledDev.has(p.name))];
}
let npm = '\n\nThird-party notices for the npm dependencies of tulkabd.\n\n';
for (const [spdx, pkgs] of Object.entries(bySpdx)) {
  for (const pkg of pkgs) {
    npm += `${'='.repeat(80)}\n${pkg.name} ${pkg.versions.join(', ')} (${spdx})\n`;
    if (pkg.author) npm += `Author: ${pkg.author}\n`;
    const dir = pkg.paths[0];
    const file = dir && readdirSync(dir).find((f) => /^(licen[sc]e|copying)(\..*)?$/i.test(f));
    npm += file
      ? `\n${readFileSync(join(dir, file), 'utf8')}\n\n`
      : '\n(no license file shipped; see package metadata)\n\n';
  }
}

writeFileSync(out, rust + npm);
console.log(`wrote ${out}`);
