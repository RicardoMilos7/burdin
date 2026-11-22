// Checks every Burdin program on the site against the compiler.
//
//   1. examples/*.bd: each program is run. Its standard output must equal examples/<name>.out, and its
//      exit status must equal examples/<name>.status (0 when that file is absent). A program's
//      standard input comes from examples/<name>.in when that file exists.
//   2. snippets/*.bd: each program must compile and run without error.
//   3. The ```burdin code blocks in src/content/docs: each block must compile. A block without
//      `fn main` gets an empty one appended, so a block of declarations is checked too. A block with
//      `test` items is run with `burdin test`; any other block is type-checked with `burdin check`.
//      A block whose info string says `fragment` (```burdin fragment) is not checked.
//
// The compiler is `burdin` on the PATH, or the path in the BURDIN environment variable.
// Usage: node scripts/check-examples.mjs

import { spawnSync } from 'node:child_process';
import { mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, extname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const burdin = process.env.BURDIN || 'burdin';
const workDir = mkdtempSync(join(tmpdir(), 'burdin-site-'));

let failures = 0;
let passed = 0;

function fail(where, message) {
	failures += 1;
	console.error(`FAIL ${where}\n${message.trimEnd()}\n`);
}

function pass() {
	passed += 1;
}

function readOptional(path) {
	try {
		return readFileSync(path, 'utf8');
	} catch {
		return undefined;
	}
}

function normalize(text) {
	return text.replace(/\r\n/g, '\n').trimEnd();
}

function listFiles(dir, predicate) {
	const found = [];
	for (const entry of readdirSync(dir)) {
		const path = join(dir, entry);
		if (statSync(path).isDirectory()) {
			found.push(...listFiles(path, predicate));
		} else if (predicate(path)) {
			found.push(path);
		}
	}
	return found.sort();
}

function runBurdin(args, input) {
	return spawnSync(burdin, args, { encoding: 'utf8', input: input ?? '', timeout: 60_000 });
}

function outputOf(result) {
	return `${result.stdout ?? ''}${result.stderr ?? ''}`;
}

// 1. Examples: run, compare the output and the exit status.
const examplesDir = join(root, 'examples');
for (const file of listFiles(examplesDir, (p) => extname(p) === '.bd')) {
	const name = relative(root, file);
	const base = join(examplesDir, basename(file, '.bd'));
	const expectedOut = readOptional(`${base}.out`);
	if (expectedOut === undefined) {
		fail(name, `missing ${basename(base)}.out with the expected standard output`);
		continue;
	}
	const expectedStatus = Number(readOptional(`${base}.status`)?.trim() ?? '0');
	const stdin = readOptional(`${base}.in`) ?? '';
	const result = runBurdin(['run', file], stdin);
	if (result.error) {
		fail(name, String(result.error));
		continue;
	}
	const actual = normalize(result.stdout ?? '');
	if (actual !== normalize(expectedOut)) {
		fail(name, `standard output differs from ${basename(base)}.out\n--- expected\n${normalize(expectedOut)}\n--- actual\n${actual}`);
	} else if (result.status !== expectedStatus) {
		fail(name, `exit status ${result.status}, expected ${expectedStatus}\n${outputOf(result)}`);
	} else {
		pass();
	}
}

// 2. Snippets: each must compile and run.
const snippetsDir = join(root, 'snippets');
for (const file of listFiles(snippetsDir, (p) => extname(p) === '.bd')) {
	const name = relative(root, file);
	const result = runBurdin(['run', file]);
	if (result.status !== 0) {
		fail(name, `exit status ${result.status}\n${outputOf(result)}`);
	} else {
		pass();
	}
}

// 3. Documentation: every ```burdin block (not marked `fragment`) must compile.
const docsDir = join(root, 'src', 'content', 'docs');
const blockPattern = /^```burdin([^\n]*)\n([\s\S]*?)^```[ \t]*$/gm;
let blockIndex = 0;
for (const file of listFiles(docsDir, (p) => /\.mdx?$/.test(p))) {
	const text = readFileSync(file, 'utf8');
	const name = relative(root, file);
	for (const match of text.matchAll(blockPattern)) {
		blockIndex += 1;
		const info = match[1].trim();
		if (info.split(/\s+/).includes('fragment')) {
			continue;
		}
		const line = text.slice(0, match.index).split('\n').length;
		const where = `${name}:${line} (code block)`;
		let source = match[2];
		const hasMain = /\bfn\s+main\s*\(/.test(source);
		const hasTests = /^\s*test\s+"/m.test(source);
		if (!hasMain) {
			source = `${source}\nfn main() {}\n`;
		}
		const path = join(workDir, `block-${blockIndex}.bd`);
		writeFileSync(path, source);
		const result = runBurdin([hasTests ? 'test' : 'check', path]);
		if (result.status !== 0) {
			fail(where, `exit status ${result.status}\n${outputOf(result)}`);
		} else {
			pass();
		}
	}
}

rmSync(workDir, { recursive: true, force: true });

if (failures > 0) {
	console.error(`${failures} check(s) failed, ${passed} passed.`);
	process.exit(1);
}
console.log(`All ${passed} Burdin programs on the site check out.`);
