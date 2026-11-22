// Generates the API reference with `burdin doc`: one static site per official package under
// public/api/<name>/, and the standard library's under public/api/std/ when the compiler can
// document it (`burdin doc --std`). The pages are the compiler's own output, so they always match
// the source; the website only links to them (src/pages/api/index.astro).
//
// The compiler is `burdin` on the PATH, or the path in the BURDIN environment variable.
// Usage: node scripts/build-api-docs.mjs

import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const packagesDir = resolve(root, '..', 'packages');
const out = join(root, 'public', 'api');
const burdin = process.env.BURDIN || 'burdin';

// The output is regenerated from scratch, so a package that was removed leaves no pages behind.
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });

function doc(args, name) {
	const result = spawnSync(burdin, ['doc', ...args, '-o', join(out, name)], { encoding: 'utf8' });
	if (result.error) {
		console.error(`error: cannot run ${burdin}: ${result.error.message}`);
		process.exit(2);
	}
	return result;
}

let failures = 0;

// The standard library is built into the compiler, so it is not documented as an ordinary package:
// `--std` documents the one this compiler carries.
const std = doc(['--std'], 'std');
if (std.status === 0) {
	console.log('std: documented');
} else {
	rmSync(join(out, 'std'), { recursive: true, force: true });
	console.log('std: skipped (this compiler cannot document the standard library; the site links to the guide page)');
}

const packages = readdirSync(packagesDir, { withFileTypes: true })
	.filter((entry) => entry.isDirectory() && existsSync(join(packagesDir, entry.name, 'burdin.toml')))
	.map((entry) => entry.name)
	.sort();

for (const name of packages) {
	const result = doc([join(packagesDir, name)], name);
	if (result.status === 0) {
		console.log(`${name}: documented`);
	} else {
		failures += 1;
		console.error(`FAIL ${name}\n${(result.stderr || result.stdout).trimEnd()}\n`);
	}
}

console.log(`\n${packages.length - failures} of ${packages.length} packages documented in ${out}`);
process.exit(failures === 0 ? 0 : 1);
