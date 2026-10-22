import { describe, expect, it } from 'vitest';
import { BenchPlanner, type SetShape } from './planner';
import type { BuildAction, BuildStep } from './buildLog';

const shape: SetShape = { bricks: 10, submodels: [{ start: 2, end: 5 }, { start: 5, end: 8 }] };

let clock = 0;
function step(seq: number, action: BuildAction, undoes: number | null = null): BuildStep {
  clock += 1;
  return {
    seq,
    ts: new Date(1_700_000_000_000 + clock * 1000).toISOString(),
    action,
    tool: action === 'place' ? 'Edit' : 'Read',
    label: '',
    target: null,
    undoes,
  };
}

const places = (n: number, from = 0) => Array.from({ length: n }, (_, i) => step(from + i, 'place'));

describe('BenchPlanner', () => {
  it('takes one brick per placement, in build order', () => {
    const p = new BenchPlanner('lead', () => shape);
    const ops = p.ingest([{ actor: 'lead', steps: [step(0, 'browse'), ...places(3, 1)] }]);
    expect(ops.map((o) => o.kind)).toEqual(['browse', 'place', 'place', 'place']);
    expect(ops.filter((o) => o.kind === 'place').map((o) => o.brick)).toEqual([0, 1, 2]);
    expect(p.standingBricks).toBe(3);
  });

  it('a pull frees exactly the brick its placement took, and the next placement retakes it', () => {
    const p = new BenchPlanner('lead', () => shape);
    p.ingest([{ actor: 'lead', steps: places(3) }]);
    const ops = p.ingest([{ actor: 'lead', steps: [step(3, 'pull', 1), step(4, 'place')] }]);
    expect(ops.map((o) => [o.kind, o.brick])).toEqual([
      ['pull', 1],
      ['place', 1],
    ]);
    expect(p.standingBricks).toBe(3);
  });

  it('re-sending a whole log never places a brick twice', () => {
    const p = new BenchPlanner('lead', () => shape);
    const log = places(4);
    p.ingest([{ actor: 'lead', steps: log }]);
    expect(p.ingest([{ actor: 'lead', steps: log }])).toEqual([]);
    expect(p.lastSeq('lead')).toBe(3);
  });

  it('a helper claims an untouched submodel and the lead builds around it', () => {
    const p = new BenchPlanner('lead', () => shape);
    // The lead's first two bricks (0, 1) are outside both submodels.
    p.ingest([{ actor: 'lead', steps: places(2) }]);
    const claim = p.ingest([{ actor: 'helper', steps: [step(0, 'place')] }]);
    expect(claim.map((o) => [o.kind, o.sub, o.brick])).toEqual([
      ['claim', 0, null],
      ['place', null, 2],
    ]);
    // The lead skips the claimed bricks 2..4 and takes 5.
    const lead = p.ingest([{ actor: 'lead', steps: places(1, 2) }]);
    expect(lead[0].brick).toBe(5);
    const attach = p.finish('helper');
    expect(attach.map((o) => [o.kind, o.sub])).toEqual([['attach', 0]]);
    // Once attached, the claim is released and its free bricks are back in play.
    expect(p.ingest([{ actor: 'lead', steps: places(1, 3) }])[0].brick).toBe(3);
  });

  it('a helper never claims a submodel the lead already started', () => {
    const p = new BenchPlanner('lead', () => shape);
    p.ingest([{ actor: 'lead', steps: places(3) }]); // brick 2 is inside submodel 0
    const ops = p.ingest([{ actor: 'helper', steps: [step(0, 'place')] }]);
    expect(ops[0]).toMatchObject({ kind: 'claim', sub: 1 });
    expect(ops[1].brick).toBe(5);
  });

  it('a full set goes to the shelf and the next set opens', () => {
    const shapes = [{ bricks: 3, submodels: [] }, { bricks: 5, submodels: [] }];
    const p = new BenchPlanner('lead', (n) => shapes[Math.min(n, 1)]);
    const ops = p.ingest([{ actor: 'lead', steps: places(4) }]);
    expect(ops.map((o) => [o.kind, o.set, o.brick])).toEqual([
      ['place', 0, 0],
      ['place', 0, 1],
      ['place', 0, 2],
      ['complete', 0, null],
      ['place', 1, 0],
    ]);
    // A pull aimed at the shelved set changes nothing on the bench.
    expect(p.ingest([{ actor: 'lead', steps: [step(4, 'pull', 1)] }])).toEqual([]);
    expect(p.standingBricks).toBe(1);
  });

  it('merges a batch across builders by time', () => {
    const p = new BenchPlanner('lead', () => shape);
    const a = step(0, 'place');
    const b = step(0, 'place');
    const c = step(1, 'place');
    const ops = p.ingest([
      { actor: 'lead', steps: [a, c] },
      { actor: 'helper', steps: [b] },
    ]);
    expect(ops.map((o) => `${o.kind}:${o.actor}`)).toEqual(['place:lead', 'claim:helper', 'place:helper', 'place:lead']);
  });
});

import { distinctOffsets, hashId } from './catalog';

describe('distinctOffsets', () => {
  it('gives every bench a different first set, even when the hashes collide', () => {
    const ids = Array.from({ length: 12 }, (_, i) => `agent-${i}`);
    const offs = distinctOffsets(ids, 12);
    const starts = ids.map((id) => (hashId(id) + (offs.get(id) ?? 0)) % 12);
    expect(new Set(starts).size).toBe(12);
  });

  it('an older bench keeps its set when a newer one opens', () => {
    const a = distinctOffsets(['x', 'y'], 12);
    const b = distinctOffsets(['x', 'y', 'z'], 12);
    expect(b.get('x')).toBe(a.get('x'));
    expect(b.get('y')).toBe(a.get('y'));
  });
});
