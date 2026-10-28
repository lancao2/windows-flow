import type { FileEvent, Side } from '@windows-flow/shared-types';

/** Caixa da bolha (px CSS) com a ponta apontando para a borda: 140 ao longo do eixo da ponta. */
export const BUBBLE_SCALE = 0.63;
export const BUBBLE_LONG = 88;
export const BUBBLE_SHORT = 76;
/** Raio do anel de progresso e seu perímetro (o centro da bolha é o círculo). */
export const RING_R = 34;
export const RING_C = 2 * Math.PI * RING_R;
/** Atraso entre a animação de saída (um PC) e a de chegada (o outro). */
export const ARRIVE_DELAY_MS = 1000;
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
  /** O item é uma pasta inteira. */
  folder: boolean;
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
        // A bandeja tem um item por raiz (o arquivo avulso ou a pasta), no último arquivo dela.
        const item: TrayFile | null = f.root_last
          ? {
              id: f.id,
              name: f.root || f.name,
              size: f.root_bytes || f.size,
              path: f.root_path ?? f.path ?? '',
              folder: f.is_dir,
            }
          : null;
        return {
          ...s,
          items: item ? [item, ...s.items.filter((i) => i.id !== f.id)].slice(0, 8) : s.items,
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

/* ---- Lista de transferências (aba Arquivos) -------------------------------------------- */

export interface TransferRow {
  dir: 'in' | 'out';
  /** Nome mostrado: o item raiz (e "+N" quando são vários arquivos avulsos). */
  name: string;
  isDir: boolean;
  bytes: number;
  /** Progresso 0..1 do lote. */
  p: number;
  state: 'active' | 'done' | 'cancelled' | 'failed';
  /** Hora de término (HH:MM) das concluídas. */
  at?: string;
}

export interface TransfersState {
  active: Partial<Record<'in' | 'out', TransferRow>>;
  history: TransferRow[];
}

export const initialTransfers: TransfersState = { active: {}, history: [] };

export const hasActiveTransfer = (s: TransfersState) => Boolean(s.active.in || s.active.out);

const clock = () => new Date().toLocaleTimeString().slice(0, 5);

function finish(s: TransfersState, dir: 'in' | 'out', state: TransferRow['state']): TransfersState {
  const row = s.active[dir];
  if (!row) return s;
  const { [dir]: _gone, ...rest } = s.active;
  void _gone;
  const done: TransferRow = { ...row, state, p: state === 'done' ? 1 : row.p, at: clock() };
  return { active: rest, history: [done, ...s.history].slice(0, 20) };
}

export function transfersReducer(s: TransfersState, f: FileEvent): TransfersState {
  const progress = f.batch_bytes > 0 ? Math.min(1, f.batch_done / f.batch_bytes) : 1;
  if (f.state === 'failed' || f.state === 'cancelled') {
    return finish(s, f.dir, f.state);
  }
  const prev = s.active[f.dir];
  const first = f.index === 0 || !prev;
  const name = first ? f.root || f.name : prev.name;
  const isDir = first ? f.is_dir : prev.isDir;
  const label = !isDir && f.count > 1 && !name.includes(' +') ? `${name} +${f.count - 1}` : name;
  const next: TransfersState = {
    ...s,
    active: {
      ...s.active,
      [f.dir]: {
        dir: f.dir,
        name: first ? label : prev.name,
        isDir,
        bytes: f.batch_bytes,
        p: progress,
        state: 'active',
      },
    },
  };
  if (f.state === 'done' && f.index + 1 >= f.count) return finish(next, f.dir, 'done');
  return next;
}
