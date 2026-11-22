// @ts-check
import { readFileSync } from 'node:fs';
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import { ironDark, ironLight } from './src/styles/code-themes.mjs';

// Where the site is published. GitHub Pages serves a project site from /<repo>/, so the deploy workflow sets
// BURDIN_SITE_BASE=/burdin/; a local build (and a custom domain later) keeps the root.
const base = process.env.BURDIN_SITE_BASE || '/';
const site = process.env.BURDIN_SITE_URL || 'https://ricardomilos7.github.io';

// The highlighter reads the same TextMate grammar as the VS Code extension, so the site and the
// editor color Burdin code the same way. The grammar is named `burdin` (`bd` is an alias).
const burdinGrammar = JSON.parse(
	readFileSync(new URL('../editors/vscode/syntaxes/burdin.tmLanguage.json', import.meta.url), 'utf8'),
);
burdinGrammar.name = 'burdin';

const repo = 'https://github.com/RicardoMilos7/burdin';

// https://astro.build/config
export default defineConfig({
	// The dev toolbar adds a script and a dozen modules to every page in development.
	devToolbar: { enabled: false },
	site,
	base,
	markdown: {
		shikiConfig: {
			langs: [burdinGrammar],
			langAlias: { bd: 'burdin' },
		},
	},
	integrations: [
		starlight({
			title: 'Burdin',
			description:
				'Burdin is an experimental native-compiled language: Rust-level memory safety, no garbage collector, and value semantics.',
			logo: {
				src: './public/logo-placeholder.svg',
				alt: 'Burdin logo (placeholder)',
			},
			favicon: '/logo-placeholder.svg',
			social: [{ icon: 'github', label: 'GitHub', href: repo }],
			components: {
				// Adds the site's main sections (Docs, Examples, API, Reference) to the top bar.
				SocialIcons: './src/components/starlight/SocialIcons.astro',
			},
			editLink: { baseUrl: `${repo}/edit/main/website/` },
			customCss: [
				'@fontsource/schibsted-grotesk/latin-400.css',
				'@fontsource/schibsted-grotesk/latin-500.css',
				'@fontsource/schibsted-grotesk/latin-700.css',
				'@fontsource/fragment-mono/latin-400.css',
				'./src/styles/theme.css',
			],
			expressiveCode: {
				themes: [ironDark, ironLight],
				shiki: { langs: [burdinGrammar], langAlias: { bd: 'burdin' } },
				useStarlightUiThemeColors: true,
				styleOverrides: {
					borderRadius: '2px',
					borderColor: 'var(--sl-color-hairline-light)',
					codeBackground: 'var(--burdin-code-bg)',
					codeFontFamily: 'var(--sl-font-mono)',
					codeFontSize: '0.86rem',
					uiFontFamily: 'var(--sl-font)',
					frames: {
						frameBoxShadowCssValue: 'none',
						editorActiveTabBackground: 'var(--burdin-code-bg)',
						editorActiveTabIndicatorTopColor: 'var(--burdin-oxide)',
						editorActiveTabIndicatorBottomColor: 'transparent',
						editorTabBarBackground: 'var(--sl-color-black)',
						terminalBackground: 'var(--burdin-code-bg)',
					},
				},
			},
			lastUpdated: false,
			pagefind: true,
			sidebar: [
				{
					label: 'Getting started',
					items: [
						'getting-started/introduction',
						'getting-started/installation',
						'getting-started/quickstart',
						'getting-started/editors',
					],
				},
				{
					label: 'Language guide',
					items: [
						'guide/basics',
						'guide/control-flow',
						'guide/functions',
						'guide/strings-and-formatting',
						'guide/structs-and-methods',
						'guide/enums-and-match',
						'guide/ownership',
						'guide/errors',
						'guide/generics-and-interfaces',
						'guide/closures-and-iterators',
						'guide/collections',
						'guide/concurrency',
						'guide/packages-and-modules',
						'guide/testing',
					],
				},
				{
					label: 'Coming from another language',
					items: [
						'coming-from/rust',
						'coming-from/go',
						'coming-from/python',
						'coming-from/c-and-cpp',
					],
				},
				{
					label: 'Examples',
					items: [
						'examples',
						'examples/fizzbuzz',
						'examples/fibonacci',
						'examples/shapes',
						'examples/rpn-calculator',
						'examples/linked-stack',
						'examples/binary-tree',
						'examples/bank-errors',
						'examples/word-count',
						'examples/tasks-and-channels',
					],
				},
				{
					label: 'Reference',
					items: [
						'reference/cli',
						{ label: 'API reference', link: '/api/' },
						'reference/standard-library',
						'reference/language-spec',
						'reference/status',
					],
				},
			],
		}),
	],
});
