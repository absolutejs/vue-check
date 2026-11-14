#!/usr/bin/env bun
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

/*
 * Generates .vue files that real projects don't contain: malformed tags,
 * every kind of attribute and directive, entities, comments and CDATA,
 * raw-text blocks, characters outside ASCII, and files cut off at any point.
 * Deterministic (a seeded generator), so a difference can be reproduced.
 *
 *   bun native/oracle/stress.ts <out-dir> [--count=3000] [--seed=1]
 *
 * Then: MODULES=<a project's node_modules> native/oracle/run.sh <out-dir> <results-dir>
 */
const out = process.argv[2];
if (!out) throw new Error('usage: bun native/oracle/stress.ts <out-dir> [--count=3000] [--seed=1]');
const option = (name: string, fallback: string) =>
	process.argv.find((value) => value.startsWith(`--${name}=`))?.split('=')[1] ?? fallback;
const count = Number(option('count', '3000'));

let state = Number(option('seed', '1')) >>> 0;
/** mulberry32 */
const random = () => {
	state = (state + 0x6d2b79f5) >>> 0;
	let value = state;
	value = Math.imul(value ^ (value >>> 15), value | 1);
	value ^= value + Math.imul(value ^ (value >>> 7), value | 61);

	return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
};
const below = (limit: number) => Math.floor(random() * limit);
const pick = <T>(items: readonly T[]): T => items[below(items.length)] as T;
const chance = (probability: number) => random() < probability;
const several = (make: () => string, most: number, separator = '') =>
	Array.from({ length: below(most + 1) }, make).join(separator);

const TAGS = [
	'div', 'span', 'p', 'a', 'ul', 'li', 'br', 'img', 'input', 'hr', 'template', 'slot', 'component', 'Comp', 'MyComp',
	'my-el', 'svg', 'math', 'foreignObject', 'desc', 'textarea', 'title', 'pre', 'script', 'style', 'table', 'tr', 'td',
	'annotation-xml', 'mi', 'mtext', 'Transition', 'keep-alive', 'teleport', 'DIV', 'tEmplate', 'é-tag', 'x1', 'x_y', 'button'
];
/* With --expressions=valid, every expression is valid code, so that the
 * template codegen is reached; without it, most aren't, for the parser. */
const valid = option('expressions', 'any') === 'valid';
const WORDS = valid
	? ['a', 'foo', 'bar', 'item', 'items', 'index', 'key', 'value', 'x.y', 'fn()', "'s'", '1 + 2', 'a ? b : c', 'foo.bar', '[a, b]', '{ a, b: 1 }', 'fn(a)', '$event', 'a = 1', 'a => a', 'Foo']
	: ['a', 'foo', 'bar', 'item', 'items', 'index', 'key', 'value', 'x.y', 'fn()', "'s'", '1 + 2', 'a ? b : c', 'é', '😀', 'in', 'of', ''];
const SPACE = [' ', '  ', '\n', '\r\n', '\t', '\n  ', ' \n ', '\f', ''];
const ENTITIES = [
	'&amp;', '&lt;', '&gt;', '&quot;', '&nbsp;', '&amp', '&lt', '&notit;', '&not', '&#65;', '&#x41;', '&#X41', '&#128;', '&#0;',
	'&#x1F600;', '&#xD800;', '&#99999999;', '&#;', '&#x;', '&unknown;', '&', '&&', '&Afr;', '&NotEqualTilde;', '&amp=', '&ampx',
	'&copy', '&copy;', '&copyx', '&CounterClockwiseContourIntegral;', '&CounterClockwiseContourIntegra', '&a', '&#', '&#x'
];
const FOR = valid
	? ['item in items', '(item, index) in items', '(value, key, index) of object', '{ a, b } in list', '({ a, b = 1 }, i) in list', '[a, , b] in list', 'n in 10', 'item of items.filter(x => x)', '(a) in b', '{ a: { b }, ...rest } in foo', 'item in item']
	: [
	'item in items', '(item, index) in items', '(value, key, index) of object', '{ a, b } in list', '({ a, b }, i) in list',
	'[a, , b] in list', ' x   in   y ', 'in', 'a in', ' in b', 'a in b in c', 'n in 10', '(a,b,c,d) in e', 'item of items.filter(x => x in y)',
	'é in 😀', '(a) in b', '((a)) in b', 'a, in b', ',a in b', '(a, b]) in c', 'item\nin\nitems', 'a  of  b', 'ofx in y', 'x iny', '', '&amp; in &lt;', 'é in a&amp;b'
];
const space = () => pick(SPACE);
const word = () => pick(WORDS);
const entity = () => pick(ENTITIES);
const value = () => (valid ? word() : several(() => (chance(0.25) ? entity() : chance(0.2) ? space() : word()), 4));
const quoted = (text: string) => {
	const kind = below(10);
	if (kind < 6) return `"${text.replaceAll('"', "'")}"`;
	if (kind < 8) return `'${text.replaceAll("'", '"')}'`;
	if (kind < 9) return text.replace(/[\s>]/g, '') || 'x';

	return pick(['""', "''", '"', "'", '"unterminated', '=x', '`x`', 'a"b', "a'b", 'a<b', 'a=b']);
};
const attributeName = () =>
	pick([
		'id', 'class', 'style', 'is', 'lang', 'src', 'setup', 'scoped', 'module', 'generic', 'vapor', 'encoding', 'key', 'ref', 'data-x',
		'v-if', 'v-else-if', 'v-else', 'v-for', 'v-show', 'v-model', 'v-model.trim.lazy', 'v-bind', 'v-bind:foo', 'v-bind:foo.sync', 'v-on:click',
		'v-on:click.stop.prevent', 'v-slot', 'v-slot:name', 'v-slot:name.with.dots', 'v-pre', 'v-pre:x', 'v-pre.y', 'v-once', 'v-html', 'v-text',
		'v-custom:arg.mod', 'v-', 'v-:arg', 'v-:[x]', ':foo', ':foo.prop', '.foo', '.foo.bar', ':', '@', '#', '.', '@click', '@click.stop',
		'@[event]', '@[event].once', ':[key]', ':[key].camel', ':[a + b]', ':[key', ':[key]x', ':[é]', ':[😀', '#default', '#name', '#[dynamic]',
		'#name.dotted', 'v-bind:[k]', 'v-on:[e].x', 'a"b', "a'b", 'a<b', '=x', 'é', '😀', 'v-é', ':é', 'ID', 'vfoo', 'v', '[x]', 'x:y', 'x.y', '1'
	]);
const attribute = () => {
	const name = attributeName();
	const kind = below(10);
	if (kind < 2) return name;
	const text = name.startsWith('v-for') || chance(0.05) ? pick(FOR) : name === 'class' ? several(() => word() + space(), 4) : name === 'is' && chance(0.5) ? `vue:${word()}` : name === 'lang' ? pick(['ts', 'html', 'pug', 'tsx', 'js', 'md', '', 'scss']) : name === 'encoding' ? pick(['text/html', 'application/xhtml+xml', 'x']) : value();

	return `${name}${chance(0.1) ? space() : ''}=${chance(0.1) ? space() : ''}${quoted(text)}`;
};
const attributes = () => several(() => (chance(0.9) ? space() || ' ' : '') + attribute(), 4);
const interpolation = () =>
	pick(['{{ ', '{{', '{{  \n', '{ {', '{{{']) + (valid ? pick([word(), word(), '', ' ']) : pick([word(), value(), '', '  ', 'a }', '} }', 'a &amp; b', "'}}'", 'a\nb'])) + pick([' }}', '}}', ' }}', '}', '', '}}}', ' }} }}']);
const comment = () =>
	pick([
		`<!--${value()}-->`, '<!-->', '<!--->', '<!---->', `<!-- ${word()} --->`, `<!--${word()}`, '<!-', '<!', `<!DOCTYPE ${word()}>`, `<![CDATA[${value()}]]>`,
		`<![CDATA[${word()}`, '<![CDAT', `<?${word()}?>`, `<?${word()}`, '<!-- @vue-expect-error -->', '<!-- @vue-ignore -->', '<!-- @vue-skip -->',
		'<!-- a -- b -->', '<!-- é😀 -->', '<!--\n-->', '<!-- <div> -->', '<!--]]>-->', '<![CDATA[-->]]>'
	]);
const oddity = () =>
	pick(['</>', '</ div>', '</#x>', '</1>', `</${pick(TAGS)}>`, `</${pick(TAGS)} >`, `</${pick(TAGS)} x="1">`, `</${pick(TAGS)}`, '<', '<<', '< div>', '<1', '<=', '</', '>', '/>', '<div<', '<div/ >', '<div / >', '<div/x>', '&', '\u0000', '﻿', ' ', ' ']);
const text = () => several(() => (chance(0.3) ? space() : chance(0.2) ? entity() : word()), 4, chance(0.5) ? ' ' : '');
const element = (depth: number): string => {
	const tag = pick(TAGS);
	const open = `<${tag}${attributes()}${chance(0.2) ? space() : ''}`;
	const kind = below(20);
	if (kind < 3) return `${open}/>`;
	if (kind < 4) return `${open}>`;
	if (kind < 5) return open;
	const close = kind < 6 ? '' : kind < 7 ? `</${pick(TAGS)}>` : kind < 8 ? `</${tag.toUpperCase()}>` : kind < 9 ? `</${tag} >` : `</${tag}>`;

	return `${open}>${children(depth + 1)}${close}`;
};
const child = (depth: number): string => {
	const kind = below(20);
	if (kind < 8 && depth < 5) return element(depth);
	if (kind < 12) return text();
	if (kind < 15) return interpolation();
	if (kind < 17) return comment();
	if (kind < 18) return oddity();

	return space();
};
function children(depth: number): string {
	return several(() => child(depth), depth < 2 ? 6 : 3);
}
const raw = () => several(() => pick([word(), space(), '<', '</', '</s', '</script', '</style', '<div>', '{{ a }}', entity(), '<!-- x -->', '</template', '<</script>', 'const a = 1 < 2;', '.a > .b {}', 'é😀']), 6, ' ');
const block = () => {
	const kind = below(20);
	if (kind < 7) {
		const lang = chance(0.15) ? ` lang=${quoted(pick(['html', 'pug', 'md', '']))}` : '';

		return `<template${lang}${chance(0.2) ? attributes() : ''}>${children(0)}${chance(0.9) ? '</template>' : pick(['', '</templat', '</template', '</', '<'])}`;
	}
	if (kind < 11) return `<script${attributes()}>${raw()}${chance(0.9) ? '</script>' : pick(['', '</scrip', '</SCRIPT>', '</script'])}`;
	if (kind < 14) return `<style${attributes()}>${raw()}${chance(0.9) ? '</style>' : ''}`;
	if (kind < 16) {
		const tag = pick(['i18n', 'docs', 'custom-block', 'Custom', 'route', 'x_y', 'é', 'title', 'textarea', 'link', 'br', 'svg', 'template-x']);

		return `<${tag}${attributes()}>${raw()}</${tag}>`;
	}
	if (kind < 17) return comment();
	if (kind < 18) return oddity();
	if (kind < 19) return element(3);

	return text();
};

rmSync(out, { force: true, recursive: true });
mkdirSync(join(out, 'node_modules'), { recursive: true });
writeFileSync(join(out, 'package.json'), '{ "private": true }\n');
for (let index = 0; index < count; index += 1) {
	let source = several(() => block() + (chance(0.8) ? pick(['\n', '\n\n', '\r\n', ' ', '']) : ''), 5);
	if (chance(0.5)) source = `<template>${children(0)}</template>\n${source}`;
	// Cut off anywhere, so every state of the tokenizer is also an end state.
	if (chance(0.25)) source = Array.from(source).slice(0, below(Array.from(source).length + 1)).join('');
	writeFileSync(join(out, `${String(index).padStart(5, '0')}.vue`), source);
}
console.log(`${count} files in ${out}`);
