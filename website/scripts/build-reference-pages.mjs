// Generates one page per diagnostic code from the repository's reference documents, so the address every
// diagnostic links to (`<URL_BASE>/errors/<code>` and `<URL_BASE>/lints/<code>`, see
// crates/burdin_diag/src/lib.rs) resolves. `docs/error-codes.md` and `docs/lints.md` stay the single source of
// truth (tests/check_error_docs.py and tests/check_lint.py check them); the pages are written to
// src/content/docs/errors/ and src/content/docs/lints/, which are not committed.
//
// The slug keeps the code's capital letter, so the address ends in `E0003`, exactly as the compiler writes it.
// Code blocks become `burdin fragment` blocks: they are checked by the tests above, not by check-examples.
//
// Usage: node scripts/build-reference-pages.mjs

import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const docsDir = resolve(root, '..', 'docs');
const repo = 'https://github.com/RicardoMilos7/burdin/blob/main';

/** Rewrites a relative link out of docs/ into one on GitHub, since the page is not in the repository tree. */
function fixLinks(text) {
	return text.replace(/\]\(\.\.\/([^)]+)\)/g, (_, path) => `](${repo}/${path})`);
}

/** Cleans one section body: drops the checker's marker comments, and exempts code blocks from check-examples. */
function clean(body) {
	return fixLinks(body)
		.replace(/^<!--.*?-->\s*\n/gm, '')
		.replace(/^```burdin\s*$/gm, '```burdin fragment')
		.trim();
}

function generate(source, letter, folder, label) {
	const out = join(root, 'src', 'content', 'docs', folder);
	rmSync(out, { recursive: true, force: true });
	mkdirSync(out, { recursive: true });
	const text = readFileSync(join(docsDir, source), 'utf8').replace(/\r\n/g, '\n');
	const parts = text.split(/^## (?=[EL]\d{4}: )/m).slice(1);
	let count = 0;
	for (const part of parts) {
		const newline = part.indexOf('\n');
		const heading = part.slice(0, newline);
		const match = /^([EL]\d{4}): (.*)$/.exec(heading);
		if (!match || match[1][0] !== letter) continue;
		const [, code, title] = match;
		const page = [
			'---',
			`title: ${JSON.stringify(`${code}: ${title}`)}`,
			`slug: ${folder}/${code}`,
			`description: ${JSON.stringify(`${label} ${code}: ${title}. What it means, an example, and how to fix it.`)}`,
			'---',
			'',
			clean(part.slice(newline + 1)),
			'',
			`See the [full ${label.toLowerCase()} reference](${repo}/docs/${source}), or run \`burdin explain ${code}\` in a terminal.`,
			'',
		].join('\n');
		writeFileSync(join(out, `${code}.md`), page);
		count += 1;
	}
	if (count === 0) {
		console.error(`error: no ${label} sections found in docs/${source}`);
		process.exit(1);
	}
	console.log(`${folder}: ${count} pages`);
}

generate('error-codes.md', 'E', 'errors', 'Error');
generate('lints.md', 'L', 'lints', 'Lint');
