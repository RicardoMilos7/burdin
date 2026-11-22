// The examples shown on the site. Each program and its expected output are imported from the files the checker
// runs (`examples/*.bd` and `examples/*.out`), so a page can only show output the compiler actually produced.

import fizzbuzzSource from '../../examples/fizzbuzz.bd?raw';
import fizzbuzzOutput from '../../examples/fizzbuzz.out?raw';
import fibonacciSource from '../../examples/fibonacci.bd?raw';
import fibonacciOutput from '../../examples/fibonacci.out?raw';
import shapesSource from '../../examples/shapes.bd?raw';
import shapesOutput from '../../examples/shapes.out?raw';
import rpnSource from '../../examples/rpn-calculator.bd?raw';
import rpnOutput from '../../examples/rpn-calculator.out?raw';
import stackSource from '../../examples/linked-stack.bd?raw';
import stackOutput from '../../examples/linked-stack.out?raw';
import treeSource from '../../examples/binary-tree.bd?raw';
import treeOutput from '../../examples/binary-tree.out?raw';
import bankSource from '../../examples/bank-errors.bd?raw';
import bankOutput from '../../examples/bank-errors.out?raw';
import wordSource from '../../examples/word-count.bd?raw';
import wordOutput from '../../examples/word-count.out?raw';
import wordInput from '../../examples/word-count.in?raw';
import tasksSource from '../../examples/tasks-and-channels.bd?raw';
import tasksOutput from '../../examples/tasks-and-channels.out?raw';

export interface Example {
	/** The page slug under /examples/. */
	slug: string;
	title: string;
	description: string;
	/** Short labels for the features the program shows. */
	features: string[];
	/** The file name the program is saved as, used in the run command. */
	file: string;
	source: string;
	output: string;
	/** Standard input, for programs that read it. */
	input?: string;
}

export const examples: Example[] = [
	{
		slug: 'fizzbuzz',
		title: 'FizzBuzz',
		description: 'A function that picks a label for each number, and a loop over a range.',
		features: ['if as a value', 'range loop'],
		file: 'fizzbuzz.bd',
		source: fizzbuzzSource,
		output: fizzbuzzOutput,
	},
	{
		slug: 'fibonacci',
		title: 'Fibonacci',
		description: 'Fibonacci numbers, computed by recursion and by a loop that keeps the last two values.',
		features: ['recursion', 'mutable locals', 'integer types'],
		file: 'fibonacci.bd',
		source: fibonacciSource,
		output: fibonacciOutput,
	},
	{
		slug: 'shapes',
		title: 'Shapes',
		description: 'Two shapes that implement one interface, a generic function, and a list that holds both.',
		features: ['interfaces', 'generic functions', 'any'],
		file: 'shapes.bd',
		source: shapesSource,
		output: shapesOutput,
	},
	{
		slug: 'rpn-calculator',
		title: 'RPN calculator',
		description: 'A reverse Polish notation calculator whose errors are values, with messages and ? that passes them up.',
		features: ['enums', 'error enums', '?'],
		file: 'rpn-calculator.bd',
		source: rpnSource,
		output: rpnOutput,
	},
	{
		slug: 'linked-stack',
		title: 'A persistent stack',
		description: 'A generic stack in which each node holds the rest of the stack in a Heap, and push and pop consume the old stack.',
		features: ['generics', 'Heap', 'own'],
		file: 'linked-stack.bd',
		source: stackSource,
		output: stackOutput,
	},
	{
		slug: 'binary-tree',
		title: 'Expression tree',
		description: 'An expression tree of boxed nodes, evaluated and printed by recursion.',
		features: ['recursive enums', 'Heap', 'match'],
		file: 'binary-tree.bd',
		source: treeSource,
		output: treeOutput,
	},
	{
		slug: 'bank-errors',
		title: 'Bank ledger with errors',
		description: 'A small ledger whose operations return Result, with an error enum, ? across several functions, and main returning Result.',
		features: ['Result', 'from', 'main returning Result'],
		file: 'bank-errors.bd',
		source: bankSource,
		output: bankOutput,
	},
	{
		slug: 'word-count',
		title: 'Word count',
		description: 'Count the words in the standard input and print them in sorted order, with a map and a line-reading loop.',
		features: ['standard input', 'Map', 'sorting'],
		file: 'word-count.bd',
		source: wordSource,
		output: wordOutput,
		input: wordInput,
	},
	{
		slug: 'tasks-and-channels',
		title: 'Tasks and channels',
		description: 'Four tasks each sum a quarter of a range and send the result through a channel.',
		features: ['task.spawn', 'Channel', 'join'],
		file: 'tasks-and-channels.bd',
		source: tasksSource,
		output: tasksOutput,
	},
];

export function bySlug(slug: string): Example {
	const found = examples.find((e) => e.slug === slug);
	if (!found) throw new Error(`unknown example: ${slug}`);
	return found;
}

/** The command that runs an example, as the reader would type it. */
export function commandFor(example: Example): string {
	return example.input === undefined
		? `burdin run ${example.file}`
		: `burdin run ${example.file} < ${example.file.replace('.bd', '.in')}`;
}
