'use strict';

/**
 * Generates the dice parity fixture from the JS reference implementation.
 * Run from E:\DnD-rs: node tools/gen-dice-fixtures.js
 *
 * The Rust test `dice_parity` replays every case and compares the exact JSON.
 */

const fs = require('fs');
const path = require('path');
const dice = require('E:/DnD/renderer/dice.js');

const CASES = [
  ['d20', [17]],
  [' d20 ', [3]],
  ['D20', [20]],
  ['2d6+3', [1, 6]],
  ['2d6-3', [6, 6]],
  ['4d6dl1', [3, 6, 2, 5]],
  ['4d6dh1', [3, 6, 2, 5]],
  ['2d20kh1', [3, 6]],
  ['2d20kh1+7', [11, 15]],
  ['2d20kl1', [3, 6]],
  ['3d8dh2', [1, 8, 4]],
  ['3d8kl2', [1, 8, 4]],
  ['1d4', [4]],
  ['50d6', Array.from({ length: 50 }, (_, i) => (i % 6) + 1)],
  ['d100', [100]],
  ['d10+999', [10]],
  ['d10-999', [1]],
  ['2 d 6 + 3', [2, 4]],
  ['2d6+1000', []],
  ['2d6-1000', []],
  ['d7', []],
  ['d0', []],
  ['d3', []],
  ['0d6', []],
  ['51d6', []],
  ['4d6dl5', []],
  ['4d6kh0', []],
  ['2d6+3x', []],
  ['d', []],
  ['d6+', []],
  ['d6-', []],
  ['', []],
  ['   ', []],
  ['2d6++3', []],
  ['-2d6', []],
  ['d6kh', []],
  ['2d6dl', []],
  ['2d6dlx', []],
  ['2d6+3 4', [2, 4]],
  ['20d100', Array.from({ length: 20 }, () => 100)],
  ['7d12', [1, 2, 3, 4, 5, 6, 7]],
  ['6d4dh3', [4, 1, 3, 2, 3, 1]],
  ['6d4kh3', [4, 1, 3, 2, 3, 1]],
  ['6d4kl3', [4, 1, 3, 2, 3, 1]],
  ['6d4dl3', [4, 1, 3, 2, 3, 1]]
];

function rollWithSequence(parsed, sequence) {
  let index = 0;
  return dice.rollFormula(parsed, () => sequence[index++ % sequence.length]);
}

const cases = CASES.map(([input, sequence]) => {
  const parsed = dice.parseFormula(input);
  if (!parsed.ok) return { input, sequence, parse: parsed };
  return {
    input,
    sequence,
    parse: parsed,
    roll: rollWithSequence(parsed, sequence)
  };
});

const output = path.join('E:/DnD-rs', 'fixtures', 'dice.json');
fs.mkdirSync(path.dirname(output), { recursive: true });
fs.writeFileSync(output, JSON.stringify(cases, null, 2), 'utf8');
console.log(`wrote ${cases.length} cases to ${output}`);
