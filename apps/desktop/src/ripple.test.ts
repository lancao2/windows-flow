import { describe, expect, it } from 'vitest';
import { MAX_RADIUS, maxRadiusFor, sideVectors, waveProgress, waveRadius } from './ripple';

describe('ondas', () => {
  it('ao entrar a onda se expande; ao sair ela se contrai até o contato', () => {
    expect(waveRadius(0, true)).toBe(0);
    expect(waveRadius(1, true)).toBe(MAX_RADIUS);
    expect(waveRadius(0, false)).toBe(MAX_RADIUS);
    expect(waveRadius(1, false)).toBe(0);
    for (const enter of [true, false]) {
      let prev = waveRadius(0, enter);
      for (let q = 0.1; q <= 1; q += 0.1) {
        const r = waveRadius(q, enter);
        expect(enter ? r >= prev : r <= prev).toBe(true);
        prev = r;
      }
    }
  });

  it('as ondas partem escalonadas e ficam em [0, 1]', () => {
    expect(waveProgress(0.2, 0)).toBeGreaterThan(waveProgress(0.2, 1));
    for (let t = 0; t <= 1; t += 0.05) {
      for (const k of [0, 1, 2]) {
        const q = waveProgress(t, k);
        expect(q).toBeGreaterThanOrEqual(0);
        expect(q).toBeLessThanOrEqual(1);
      }
    }
  });
});

describe('maxRadiusFor', () => {
  it('mantém as ondas dentro da janela', () => {
    expect(maxRadiusFor(440)).toBeLessThan(440);
    expect(maxRadiusFor(2000)).toBe(MAX_RADIUS);
    expect(waveRadius(1, true, maxRadiusFor(440))).toBeLessThan(440);
  });
});

describe('sideVectors', () => {
  const W = 440;
  const H = 1000;
  /** Posição em pixels do ponto local (u, v). */
  const at = (side: 'left' | 'right' | 'top' | 'bottom', u: number, v: number) => {
    const s = sideVectors(
      side,
      side === 'left' || side === 'right' ? W : H,
      side === 'left' || side === 'right' ? H : W,
    );
    return [s.o[0] + s.u[0] * u + s.v[0] * v, s.o[1] + s.u[1] * u + s.v[1] * v];
  };

  it('u = 0 cai sobre a borda correta e u > 0 entra na tela', () => {
    expect(at('left', 0, 300)).toEqual([0, 300]);
    expect(at('left', 100, 300)).toEqual([100, 300]);
    expect(at('right', 0, 300)).toEqual([W, 300]);
    expect(at('right', 100, 300)).toEqual([W - 100, 300]);
    expect(at('top', 0, 300)).toEqual([300, 0]);
    expect(at('top', 100, 300)).toEqual([300, 100]);
    expect(at('bottom', 0, 300)).toEqual([300, W]);
    expect(at('bottom', 100, 300)).toEqual([300, W - 100]);
  });
});
