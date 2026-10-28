import { describe, expect, it } from 'vitest';
import { linkLabel, looksLikePeerAddress, nearestSide, sideLabel, stateLabel } from './status';

describe('labels', () => {
  it('traduz estados', () => {
    expect(stateLabel('local')).toBe('Controlando este PC');
    expect(stateLabel('remote')).toBe('Controlando o outro PC');
    expect(linkLabel('connected')).toBe('Conectado');
    expect(sideLabel('left')).toBe('à esquerda');
  });
});

describe('looksLikePeerAddress', () => {
  it('aceita IP com ou sem porta', () => {
    expect(looksLikePeerAddress('192.168.0.10')).toBe(true);
    expect(looksLikePeerAddress(' 10.0.0.2:47800 ')).toBe(true);
  });
  it('rejeita entradas inválidas', () => {
    for (const s of ['', 'abc', '300.1.1.1', '1.1.1.1:0', '1.1.1.1:70000', '1.1.1']) {
      expect(looksLikePeerAddress(s)).toBe(false);
    }
  });
});

describe('nearestSide', () => {
  it('escolhe o lado pelo eixo dominante', () => {
    expect(nearestSide(120, 10)).toBe('right');
    expect(nearestSide(-120, -10)).toBe('left');
    expect(nearestSide(5, -90)).toBe('top');
    expect(nearestSide(-5, 90)).toBe('bottom');
    expect(nearestSide(0, 0)).toBe('right');
  });
});
