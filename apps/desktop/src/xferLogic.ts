import type { FileEvent, Side } from '@windows-flow/shared-types';

/** Caixa da bolha (px CSS) com a ponta apontando para a borda: 140 ao longo do eixo da ponta. */
export const BUBBLE_LONG = 140;
export const BUBBLE_SHORT = 120;
/** Raio do anel de progresso e seu perímetro (o centro da bolha é o círculo). */
export const RING_R = 34;
export const RING_C = 2 * Math.PI * RING_R;
/** Atraso entre a animação de saída (um PC) e a de chegada (o outro). */
export const ARRIVE_DELAY_MS = 900;
/** Tempo mínimo com o anel visível no receptor, mesmo para arquivos pequenos. */
export const MIN_RING_MS = 1700;

const ROTATION: Record<Side, number> = { right: 0, bottom: 90, left: 180, top: 270 };

const clamp = (x: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, x));

export interface BubbleBox {
  left: number;
  top: number;
  width: number;
  height: number;
  rotate: number;
}

/** Retângulo da bolha rente à borda `side`, centrada em `v` ao longo dela, numa janela w x h. */
export function bubbleBox(side: Side, v: number, w: number, h: number): BubbleBox {
  const horizontal = side === 'left' || side === 'right';
  const [bw, bh] = horizontal ? [BUBBLE_LONG, BUBBLE_SHORT] : [BUBBLE_SHORT, BUBBLE_LONG];
  const top = horizontal ? clamp(v - bh / 2, 0, Math.max(0, h - bh)) : side === 'top' ? 0 : h - bh;
  const left = horizontal
    ? side === 'left'
      ? 0
      : w - bw
    : clamp(v - bw / 2, 0, Math.max(0, w - bw));
  return { left, top, width: bw, height: bh, rotate: ROTATION[side] };
}

/** `stroke-dashoffset` do anel para o progresso `p` (0..1). */
export const ringOffset = (p: number) => RING_C * (1 - clamp(p, 0, 1));

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 || Number.isInteger(v) ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

export interface TrayFile {
  id: number;
  name: string;
  size: number;
  path: string;
}

export interface RecvState {
  /** hidden: nada; wait: chegou, aguardando a vez da animação; ring: anel; tray: bandeja. */
  phase: 'hidden' | 'wait' | 'ring' | 'tray';
  /** Lote em andamento (anel/linha de progresso). */
  active: { name: string; p: number; index: number; count: number } | null;
  /** O último arquivo do lote já chegou. */
  finished: boolean;
  items: TrayFile[];
}

export const initialRecv: RecvState = { phase: 'hidden', active: null, finished: false, items: [] };

export type RecvAction =
  | { t: 'file'; f: FileEvent }
  | { t: 'show' }
  | { t: 'tray' }
  | { t: 'remove'; id: number }
  | { t: 'dismiss' };

const progress = (f: FileEvent) =>
  f.batch_bytes > 0 ? Math.min(1, f.batch_done / f.batch_bytes) : 1;

export function recvReducer(s: RecvState, a: RecvAction): RecvState {
  switch (a.t) {
    case 'file': {
      const f = a.f;
      if (f.dir !== 'in') return s;
      if (f.state === 'offer') {
        return {
          ...s,
          phase: s.phase === 'hidden' ? 'wait' : s.phase,
          active: { name: f.name, p: progress(f), index: f.index, count: f.count },
          finished: false,
        };
      }
      if (f.state === 'progress') {
        return s.active ? { ...s, active: { ...s.active, p: progress(f) } } : s;
      }
      if (f.state === 'done') {
        const last = f.index + 1 >= f.count;
        const item: TrayFile = { id: f.id, name: f.name, size: f.size, path: f.path ?? '' };
        return {
          ...s,
          items: [item, ...s.items.filter((i) => i.id !== f.id)].slice(0, 8),
          active: last ? null : s.active && { ...s.active, p: progress(f) },
          finished: last,
        };
      }
      // failed: o anel some; se já há arquivos na bandeja, ela continua
      return { ...s, active: null, finished: false, phase: s.items.length ? 'tray' : 'hidden' };
    }
    case 'show':
      return s.phase === 'wait' ? { ...s, phase: 'ring' } : s;
    case 'tray':
      return s.items.length ? { ...s, phase: 'tray' } : { ...s, phase: 'hidden' };
    case 'remove': {
      const items = s.items.filter((i) => i.id !== a.id);
      return { ...s, items, phase: items.length || s.active ? s.phase : 'hidden' };
    }
    case 'dismiss':
      return initialRecv;
  }
}
