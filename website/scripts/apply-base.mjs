// Rewrites the site's own absolute links to carry the base path, for a deployment served from a subdirectory
// (GitHub Pages serves a project site at /<repo>/). Astro applies `base` to its routes and components, and the
// Markdown processor does most of it too, but a hand-written link can still come out as `/guide/ownership/`.
// This is the last pass: it only touches the files `astro build` wrote, and with the default base of `/` it does
// nothing.
//
// Usage: node scripts/apply-base.mjs [base]   (or BURDIN_SITE_BASE)

import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const dist = join(root, 'dist');
const base = (process.argv[2] || process.env.BURDIN_SITE_BASE || '/').replace(/\/+$/, '');

if (!base) {
	console.log('apply-base: base is `/`, nothing to do');
	process.exit(0);
}

/** Every file under `dir`, recursively. */
function files(dir) {
	return readdirSync(dir).flatMap((name) => {
		const path = join(dir, name);
		return statSync(path).isDirectory() ? files(path) : [path];
	});
}

// `href`, `src`, and `url(...)` in a style attribute or stylesheet. A `//host` address, an address that already
// carries the base, and a fragment or a relative path are left alone.
const attribute = /(\b(?:href|src)=")(\/(?!\/)[^"]*)"/g;
const css = /(url\()(\/(?!\/)[^)'"]*)/g;

let changed = 0;
for (const path of files(dist)) {
	if (!/\.(html|css)$/.test(path)) continue;
	const text = readFileSync(path, 'utf8');
	const prefix = (url) => (url.startsWith(`${base}/`) ? url : base + url);
	const out = text.replace(attribute, (_, start, url) => `${start}${prefix(url)}"`).replace(css, (_, start, url) => `${start}${prefix(url)}`);
	if (out !== text) {
		writeFileSync(path, out);
		changed += 1;
	}
}

console.log(`apply-base: base \`${base}/\` applied to ${changed} file${changed === 1 ? '' : 's'}`);
