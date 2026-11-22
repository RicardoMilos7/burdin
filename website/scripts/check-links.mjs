// Checks every link in the built site (`dist/`): each `href` and `src` that points inside the site must lead to a
// file, and each `#fragment` that names an id on the page must find it. Used after a build to catch a broken link
// before it is published.
//
// Usage: node scripts/check-links.mjs [base]

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const dist = join(root, 'dist');
const base = (process.argv[2] || process.env.BURDIN_SITE_BASE || '/').replace(/\/+$/, '');

/** Every HTML file under `dir`. */
function pages(dir) {
	const out = [];
	for (const name of readdirSync(dir)) {
		const path = join(dir, name);
		if (statSync(path).isDirectory()) out.push(...pages(path));
		else if (path.endsWith('.html')) out.push(path);
	}
	return out;
}

/** The page file an address names, or `null`. A directory address maps to its `index.html`. */
function target(page, url) {
	const path = url.split('#')[0].split('?')[0];
	if (!path) return page;
	// Served from a subdirectory, an internal address from the site root must carry the base, or it points outside
	// the site when it is published.
	if (base && path.startsWith('/') && !path.startsWith(`${base}/`)) return null;
	let file;
	if (path.startsWith('/')) {
		const withoutBase = base && path.startsWith(`${base}/`) ? path.slice(base.length) : path;
		file = join(dist, withoutBase);
	} else {
		file = resolve(dirname(page), path);
	}
	for (const candidate of [file, join(file, 'index.html')]) {
		if (existsSync(candidate) && statSync(candidate).isFile()) return candidate;
	}
	return null;
}

let links = 0;
let broken = 0;
for (const page of pages(dist)) {
	const html = readFileSync(page, 'utf8');
	const ids = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
	for (const [, url] of html.matchAll(/\b(?:href|src)="([^"]*)"/g)) {
		if (/^(?:[a-z]+:|\/\/|#|mailto:)/i.test(url)) {
			// A `#fragment` on the page itself still has to name an id.
			if (url.startsWith('#') && url.length > 1 && !ids.has(url.slice(1))) {
				broken += 1;
				console.error(`BROKEN ${relative(dist, page)}: ${url} names no id`);
			}
			continue;
		}
		links += 1;
		const file = target(page, url);
		if (!file) {
			broken += 1;
			console.error(`BROKEN ${relative(dist, page).split(sep).join('/')}: ${url}`);
			continue;
		}
		const fragment = url.includes('#') ? url.split('#')[1] : '';
		if (fragment && file.endsWith('.html')) {
			const targetIds = new Set([...readFileSync(file, 'utf8').matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
			if (!targetIds.has(fragment)) {
				broken += 1;
				console.error(`BROKEN ${relative(dist, page).split(sep).join('/')}: ${url} names no id on ${relative(dist, file)}`);
			}
		}
	}
}

console.log(`${links} internal links, ${broken} broken`);
process.exit(broken === 0 ? 0 : 1);
