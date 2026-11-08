// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import { wheelSteps } from './wheel';

const px = (deltaY: number) => ({ deltaX: 0, deltaY, deltaMode: 0 });
const lines = (deltaY: number) => ({ deltaX: 0, deltaY, deltaMode: 1 });

describe('wheelSteps', () => {
  it('makes one step per mouse notch', () => {
    expect(wheelSteps(0, px(100))).toEqual({ steps: 1, carry: 0 });
    expect(wheelSteps(0, px(-100))).toEqual({ steps: -1, carry: 0 });
    expect(wheelSteps(0, lines(3))).toEqual({ steps: 1, carry: 0 });
  });

  it('adds up small touchpad deltas', () => {
    let carry = 0;
    let steps = 0;
    for (let i = 0; i < 10; i++) {
      const out = wheelSteps(carry, px(20));
      carry = out.carry;
      steps += out.steps;
    }
    expect(steps).toBe(2);
  });

  it('never lets a half notch do nothing', () => {
    expect(wheelSteps(0, px(60)).steps).toBe(1);
    expect(wheelSteps(0, px(10)).steps).toBe(0);
  });

  it('drops what was left over when the direction changes', () => {
    const up = wheelSteps(0, px(40));
    expect(up).toEqual({ steps: 0, carry: 40 });
    expect(wheelSteps(up.carry, px(-40))).toEqual({ steps: 0, carry: -40 });
  });

  it('ignores an event that does not move', () => {
    expect(wheelSteps(30, px(0))).toEqual({ steps: 0, carry: 30 });
  });
});
