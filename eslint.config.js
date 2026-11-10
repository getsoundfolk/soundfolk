// SPDX-License-Identifier: MPL-2.0
// Lint rules for the front end. Formatting is Prettier's job, types are svelte-check's.
import js from '@eslint/js';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import ts from 'typescript-eslint';
import svelteConfig from './svelte.config.js';

export default ts.config(
  { ignores: ['dist/', 'node_modules/', 'src-tauri/', 'src/lib/bindings.ts'] },
  js.configs.recommended,
  ...ts.configs.recommended,
  ...svelte.configs.recommended,
  {
    languageOptions: { globals: { ...globals.browser } },
  },
  {
    files: ['**/*.svelte', '**/*.svelte.ts'],
    languageOptions: {
      parserOptions: {
        projectService: true,
        extraFileExtensions: ['.svelte'],
        parser: ts.parser,
        svelteConfig,
      },
    },
  },
  {
    rules: {
      // `_name` marks an argument kept on purpose.
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
      // A failure is logged or shown (errors.report). One that does not matter says why, in an
      // `eslint-disable-next-line` comment (docs/99-analysis.md §2.4).
      'no-restricted-syntax': [
        'error',
        {
          selector:
            "CallExpression[callee.property.name='catch'] > :function[params.length=0].arguments",
          message:
            'Swallowed error: report it, or say why it does not matter in an eslint-disable comment.',
        },
      ],
    },
  },
);
