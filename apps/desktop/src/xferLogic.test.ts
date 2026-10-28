import { describe, expect, it } from 'vitest';
import type { FileEvent } from '@windows-flow/shared-types';
import {
  RING_C,
  bubbleBox,
  formatBytes,
  initialRecv,
  recvReducer,
  ringOffset,
  type RecvState,
} from './xferLogic';

const ev = (over: Partial<FileEvent>): FileEvent => ({
  dir: 'in',
  state: 'offer',
  id: 1,
  name: 'a.txt',
  size: 100,
  index: 0,
  count: 1,
  batch_done: 0,
  batch_bytes: 100,
  ratio: 5000,
  side: 'left',
  screen: [0, 0, 1920, 1080],
  path: null,
  root: 'a.txt',
  root_path: null,
  root_bytes: 100,
  root_last: false,
  is_dir: false,
  ...over,
});

const run = (events: FileEvent[], from: RecvState = initialRecv) =>
  events.reduce((s, f) => recvReducer(s, { t: 'file', f }), from);

describe('bubbleBox', () => {
  it('encosta a ponta na borda de cada lado, girando a bolha', () => {
    expect(bubbleBox('right', 200, 400, 400)).toMatchObject({ left: 312, top: 162, rotate: 0 });
    expect(bubbleBox('left', 200, 400, 400)).toMatchObject({ left: 0, top: 162, rotate: 180 });
    expect(bubbleBox('top', 200, 400, 400)).toMatchObject({ left: 162, top: 0, rotate: 270 });
    expect(bubbleBox('bottom', 200, 400, 400)).toMatchObject({ left: 162, top: 312, rotate: 90 });
  });

  it('mantém a bolha dentro da janela quando o contato está perto do canto', () => {
    expect(bubbleBox('right', -50, 400, 400).top).toBe(0);
    expect(bubbleBox('right', 999, 400, 400).top).toBe(324);
  });
});

describe('ringOffset', () => {
  it('vai de anel vazio a completo', () => {
    expect(ringOffset(0)).toBeCloseTo(RING_C);
    expect(ringOffset(1)).toBe(0);
    expect(ringOffset(2)).toBe(0);
    expect(ringOffset(-1)).toBeCloseTo(RING_C);
  });
});

describe('formatBytes', () => {
  it('usa a unidade certa', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5 MB');
    expect(formatBytes(3.2 * 1024 ** 3)).toBe('3.2 GB');
  });
});

describe('recvReducer', () => {
  it('oferta abre a espera; show mostra o anel; progresso atualiza o percentual', () => {
    let s = run([ev({})]);
    expect(s.phase).toBe('wait');
    s = recvReducer(s, { t: 'show' });
    expect(s.phase).toBe('ring');
    s = run([ev({ state: 'progress', batch_done: 40 })], s);
    expect(s.active?.p).toBeCloseTo(0.4);
  });

  it('último arquivo concluído marca o lote como terminado e entra na bandeja', () => {
    let s = run([
      ev({}),
      ev({
        state: 'done',
        batch_done: 100,
        path: 'C:\\x\\a.txt',
        root_last: true,
        root_path: 'C:\\x\\a.txt',
      }),
    ]);
    s = recvReducer(recvReducer(s, { t: 'show' }), { t: 'tray' });
    expect(s).toMatchObject({ phase: 'tray', finished: true, active: null });
    expect(s.items.map((i) => i.name)).toEqual(['a.txt']);
  });

  it('pasta com vários arquivos vira um único item na bandeja', () => {
    const f = (i: number, over: Partial<FileEvent> = {}) =>
      ev({
        id: i,
        index: i,
        count: 3,
        batch_bytes: 90,
        root: 'Projeto',
        root_bytes: 90,
        is_dir: true,
        ...over,
      });
    let s = run([f(0), f(0, { state: 'done', batch_done: 30 })]);
    s = run([f(1), f(1, { state: 'done', batch_done: 60 })], s);
    expect(s.items).toHaveLength(0);
    const last = f(2, {
      state: 'done',
      batch_done: 90,
      root_last: true,
      root_path: 'C:\\d\\Projeto',
    });
    s = run([f(2), last], s);
    expect(s.items).toEqual([
      { id: 2, name: 'Projeto', size: 90, path: 'C:\\d\\Projeto', folder: true },
    ]);
    expect(s.finished).toBe(true);
  });

  it('lote com vários arquivos só termina no último', () => {
    const s = run([
      ev({ count: 2, batch_bytes: 200 }),
      ev({ state: 'done', count: 2, batch_bytes: 200, batch_done: 100 }),
    ]);
    expect(s.finished).toBe(false);
    expect(s.active?.p).toBeCloseTo(0.5);
  });

  it('falha sem arquivos esconde tudo; com arquivos mantém a bandeja', () => {
    expect(run([ev({}), ev({ state: 'failed' })]).phase).toBe('hidden');
    const withItem = run([ev({}), ev({ state: 'done', batch_done: 100, root_last: true })]);
    const s = recvReducer(withItem, { t: 'tray' });
    expect(run([ev({ id: 2 }), ev({ id: 2, state: 'failed' })], s).phase).toBe('tray');
  });

  it('pegar o último arquivo da bandeja a esconde', () => {
    let s = run([ev({}), ev({ state: 'done', batch_done: 100, root_last: true })]);
    s = recvReducer(s, { t: 'tray' });
    expect(recvReducer(s, { t: 'remove', id: 1 }).phase).toBe('hidden');
  });

  it('ignora eventos de envio', () => {
    expect(run([ev({ dir: 'out' })]).phase).toBe('hidden');
  });
});
