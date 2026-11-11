// SPDX-License-Identifier: MPL-2.0
// CSS lint: the rules of docs/07-visual-design.md, plus stylelint's bug catchers. Formatting is
// Prettier's job. A rule broken on purpose says why: `/* stylelint-disable-next-line rule -- why */`.
import soundfolk from './stylelint-plugin-soundfolk.js';

// §5: radii on the scale, one to four of them.
const RADIUS = String.raw`(0|0\.375rem|0\.5rem|0\.75rem|999px|50%)`;
// §3: the size scale.
const SIZE = String.raw`(0\.6875|0\.75|0\.8125|0\.875|0\.9375|1|1\.125|1\.25)rem`;
// Any property but a custom one, where literal values live (the tokens of src/app.css), or a mask,
// whose colors only carry an alpha.
const NOT_TOKEN = '/^(?!--|mask)/';

/** @type {import('stylelint').Config} */
export default {
  extends: ['stylelint-config-recommended'],
  plugins: [soundfolk],
  reportDescriptionlessDisables: true,
  reportInvalidScopeDisables: true,
  reportNeedlessDisables: true,
  ignoreFiles: ['dist/**', 'node_modules/**', 'src-tauri/**', 'logo-exploration/**'],
  overrides: [{ files: ['**/*.svelte'], customSyntax: 'postcss-html' }],
  rules: {
    // §2: colors only through --color-* tokens.
    'declaration-property-value-disallowed-list': [
      {
        [NOT_TOKEN]: [
          /#[0-9a-f]{3,8}\b/i,
          /\b(rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(/i,
          /\b(white|black|red|green|blue|orange|yellow|gr[ae]y|silver|purple|pink|brown)\b/i,
        ],
        // §7: durations only through --duration-* tokens.
        '/^(transition|animation)(-duration|-delay)?$/': [/(^|[\s,(])\d*\.?\d+m?s\b/],
        // §3: no uppercase labels.
        'text-transform': ['uppercase'],
      },
      {
        message: (prop) =>
          prop === 'text-transform'
            ? 'No uppercase labels (docs/07-visual-design.md §3)'
            : `${prop}: use a var(--…) token (docs/07-visual-design.md §2, §7)`,
      },
    ],
    'declaration-property-value-allowed-list': [
      {
        // §5: radii on the scale.
        '/radius$/': [
          new RegExp(String.raw`^(var\(--[\w-]+\)|inherit|${RADIUS}( ${RADIUS}){0,3})$`),
        ],
        // §3: two weights. The players' leading line (600) says so where it is used.
        'font-weight': ['400', '500', 'inherit'],
        // §3: the size scale.
        'font-size': [new RegExp(String.raw`^(var\(--[\w-]+\)|inherit|${SIZE})$`)],
      },
      { message: (prop) => `${prop}: off the scale of docs/07-visual-design.md §3, §5` },
    ],
    // Order-based specificity checks misfire on CSS Modules, where each class is its own component
    // part; the cascade is read rule by rule there.
    'no-descending-specificity': null,
    'soundfolk/reduced-motion': true,
    'soundfolk/scrollbar-gutter': true,
  },
};
