// The two syntax themes the site uses for every code block. They follow the page palette: graphite
// text, keywords in the iron-oxide accent, strings in blued steel, and comments in a muted gray.
// Names and types stay in the text color, so a block reads mostly as ink with a few marks of color.
// The scopes are the generic TextMate ones, so shell, TOML, and Rust blocks use the same colors.

/** @param {Record<string, string>} c */
function tokens(c) {
	return [
		{ scope: ['comment', 'punctuation.definition.comment'], settings: { foreground: c.comment } },
		{
			scope: ['keyword', 'storage', 'storage.type', 'storage.modifier', 'keyword.control', 'keyword.declaration'],
			settings: { foreground: c.keyword },
		},
		{ scope: ['keyword.operator', 'punctuation'], settings: { foreground: c.punct } },
		{ scope: ['string', 'string.quoted', 'constant.character.escape'], settings: { foreground: c.string } },
		{
			scope: ['meta.interpolation', 'punctuation.section.interpolation'],
			settings: { foreground: c.text },
		},
		{ scope: ['punctuation.section.interpolation.begin', 'punctuation.section.interpolation.end'], settings: { foreground: c.keyword } },
		{ scope: ['constant.numeric', 'constant.language', 'constant.character'], settings: { foreground: c.number } },
		{
			scope: ['entity.name.type', 'support.type', 'entity.name.function', 'entity.name'],
			settings: { foreground: c.strong },
		},
		{ scope: ['variable.language.self', 'variable.language'], settings: { foreground: c.self } },
		{ scope: ['invalid'], settings: { foreground: c.keyword, fontStyle: 'underline' } },
	];
}

export const ironDark = {
	name: 'burdin-iron-dark',
	type: 'dark',
	colors: { 'editor.background': '#171b1e', 'editor.foreground': '#d4dadc' },
	tokenColors: tokens({
		text: '#d4dadc',
		strong: '#f1f4f4',
		comment: '#7c878c',
		keyword: '#ff8f52',
		punct: '#8e999e',
		string: '#9fc0cd',
		number: '#efc6a3',
		self: '#bcc5c8',
	}),
};

export const ironLight = {
	name: 'burdin-iron-light',
	type: 'light',
	colors: { 'editor.background': '#e3e7e6', 'editor.foreground': '#262d30' },
	tokenColors: tokens({
		text: '#262d30',
		strong: '#111518',
		comment: '#5e686c',
		keyword: '#a83806',
		punct: '#5d676c',
		string: '#285670',
		number: '#86430f',
		self: '#454f53',
	}),
};
