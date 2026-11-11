// SPDX-License-Identifier: MPL-2.0
// Rules of docs/07-visual-design.md that stylelint's own rules cannot express.
import stylelint from 'stylelint';

const {
  createPlugin,
  utils: { report, ruleMessages },
} = stylelint;

/** The selectors of a rule, one by one, without their spacing. */
function selectors(rule) {
  return rule.selector.split(',').map((s) => s.trim().replace(/\s+/g, ' '));
}

/** Whether a node sits inside an at-rule whose parameters match `pattern`. */
function inside(node, pattern) {
  for (let parent = node.parent; parent; parent = parent.parent) {
    if (parent.type === 'atrule' && pattern.test(parent.params)) return true;
  }
  return false;
}

const REDUCED = /prefers-reduced-motion:\s*reduce/;
const MOTION = /^(transition|animation)(-name|-duration)?$/;

// §7: every animation or transition is removed under `prefers-reduced-motion: reduce`, by a rule
// with the same selector in a media query of that file.
const reducedMotionName = 'soundfolk/reduced-motion';
const reducedMotionMessages = ruleMessages(reducedMotionName, {
  missing: (selector, prop) =>
    `"${selector}" sets ${prop} with no fallback: add it to a @media (prefers-reduced-motion: reduce) block that sets ${prop}: none (docs/07-visual-design.md §7)`,
});

const reducedMotion = (enabled) => (root, result) => {
  if (!enabled) return;
  const stilled = new Set();
  root.walkDecls(MOTION, (decl) => {
    if (decl.parent?.type !== 'rule' || !inside(decl, REDUCED)) return;
    const kind = decl.prop.startsWith('animation') ? 'animation' : 'transition';
    for (const selector of selectors(decl.parent)) stilled.add(`${kind} ${selector}`);
  });
  root.walkDecls(MOTION, (decl) => {
    if (decl.parent?.type !== 'rule' || inside(decl, REDUCED)) return;
    if (/^(none|0m?s)$/.test(decl.value.trim())) return;
    const kind = decl.prop.startsWith('animation') ? 'animation' : 'transition';
    for (const selector of selectors(decl.parent)) {
      if (stilled.has(`${kind} ${selector}`)) continue;
      report({
        ruleName: reducedMotionName,
        result,
        node: decl,
        message: reducedMotionMessages.missing(selector, kind),
      });
    }
  });
};
reducedMotion.ruleName = reducedMotionName;
reducedMotion.messages = reducedMotionMessages;

// §5: a panel that scrolls vertically keeps its scrollbar's room (`scrollbar-gutter: stable`) or
// shows none (`scrollbar-width: none`), so its width does not change with its content.
const scrollbarName = 'soundfolk/scrollbar-gutter';
const scrollbarMessages = ruleMessages(scrollbarName, {
  missing: () =>
    'A panel that scrolls needs scrollbar-gutter: stable, or scrollbar-width: none (docs/07-visual-design.md §5)',
});

const scrollbarGutter = (enabled) => (root, result) => {
  if (!enabled) return;
  root.walkDecls(/^overflow(-y)?$/, (decl) => {
    const values = decl.value.trim().split(/\s+/);
    // `overflow: hidden auto` scrolls on y, its second value.
    const y = decl.prop === 'overflow' ? values[values.length - 1] : values[0];
    if (y !== 'auto' && y !== 'scroll') return;
    let kept = false;
    decl.parent?.walkDecls((other) => {
      if (other.prop === 'scrollbar-gutter' && other.value.startsWith('stable')) kept = true;
      if (other.prop === 'scrollbar-width' && other.value === 'none') kept = true;
    });
    if (!kept) {
      report({ ruleName: scrollbarName, result, node: decl, message: scrollbarMessages.missing() });
    }
  });
};
scrollbarGutter.ruleName = scrollbarName;
scrollbarGutter.messages = scrollbarMessages;

export default [
  createPlugin(reducedMotionName, reducedMotion),
  createPlugin(scrollbarName, scrollbarGutter),
];
