import { useEffect, useReducer, useRef, useState, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import type { Side, XferPlacement, XferRecvEvent, XferSendEvent } from '@windows-flow/shared-types';
import { xfer } from './api';
import {
  ARRIVE_DELAY_MS,
  MIN_RING_MS,
  RING_C,
  RING_R,
  bubbleBox,
  formatBytes,
  initialRecv,
  recvReducer,
  ringOffset,
  type TrayFile,
} from './xferLogic';

/** Largura (px CSS) da bandeja de arquivos recebidos e altura de cada linha. */
const TRAY_W = 264;
const ROW_H = 52;
const MAX_ROWS = 3;

// Balão com a ponta virada para a borda: círculo r=52 em (60,60) e ponta em (137,60).
const SHAPE = 'M137 60 L95.1 21.7 A52 52 0 1 0 95.1 98.3 Z';

function Ripples({ tag }: { tag: string }) {
  return (
    <div className="xf-ripples" key={tag} aria-hidden>
      <span className="xf-glow" />
      <span className="xf-ripple" style={{ animationDelay: '0ms' }} />
      <span className="xf-ripple" style={{ animationDelay: '220ms' }} />
      <span className="xf-ripple" style={{ animationDelay: '440ms' }} />
    </div>
  );
}

interface BubbleProps {
  side: Side;
  v: number;
  w: number;
  h: number;
  p: number;
  /** Classe de animação do balão: xf-grow, xf-shrink, xf-commit, xf-arrive, xf-done... */
  anim: string;
  /** Conteúdo no centro do anel (já girado de volta para ficar legível). */
  center: ReactNode;
  ripples?: string;
}

function Bubble({ side, v, w, h, p, anim, center, ripples }: BubbleProps) {
  const box = bubbleBox(side, v, w, h);
  return (
    <div
      className="xf-bubble"
      style={{ left: box.left, top: box.top, width: box.width, height: box.height }}
    >
      <div className="xf-rot" style={{ transform: `rotate(${box.rotate}deg)` }}>
        {ripples && <Ripples tag={ripples} />}
        <div className={`xf-inner ${anim}`}>
          <svg viewBox="0 0 140 120" width={140} height={120}>
            <path d={SHAPE} className="xf-shape" />
            <circle cx={60} cy={60} r={RING_R} className="xf-track" />
            <circle
              cx={60}
              cy={60}
              r={RING_R}
              className="xf-arc"
              strokeDasharray={RING_C}
              strokeDashoffset={ringOffset(p)}
            />
          </svg>
          <div
            className="xf-center"
            style={{ transform: `translate(-50%, -50%) rotate(${-box.rotate}deg)` }}
          >
            {center}
          </div>
        </div>
      </div>
    </div>
  );
}

const Arrow = ({ inward = false }: { inward?: boolean }) => (
  <svg viewBox="0 0 24 24" width={26} height={26} className={inward ? 'xf-flip' : undefined}>
    <path
      d="M4 12h15M13 5.5 19.5 12 13 18.5"
      fill="none"
      stroke="#fff"
      strokeWidth={2.6}
      strokeLinecap="round"
      strokeLinejoin="round"
    />
  </svg>
);

const Check = () => (
  <svg viewBox="0 0 24 24" width={28} height={28}>
    <path
      d="m5 12.5 4.5 4.5L19 7.5"
      fill="none"
      stroke="#fff"
      strokeWidth={2.8}
      strokeLinecap="round"
      strokeLinejoin="round"
    />
  </svg>
);

function useTransparentBody() {
  useEffect(() => {
    document.body.classList.add('xfer-body');
    return () => document.body.classList.remove('xfer-body');
  }, []);
}

type SendMode = 'idle' | 'hold' | 'commit' | 'cancel';

/**
 * Janela do PC que envia. Enquanto o usuário arrasta arquivos até a borda, o anel se
 * completa em 2 s (o relógio fica no backend); ao completar, o balão pulsa, emite ondas como
 * no AirDrop e "voa" para dentro da borda, rumo ao outro PC.
 */
export function XferSend() {
  useTransparentBody();
  const [place, setPlace] = useState<XferPlacement | null>(null);
  const [mode, setMode] = useState<SendMode>('idle');
  const [p, setP] = useState(0);
  const [v, setV] = useState(0);
  const timer = useRef<number>();

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen<XferSendEvent>('xfer-send', (e) => {
      const m = e.payload;
      window.clearTimeout(timer.current);
      switch (m.kind) {
        case 'catch':
          if (m.placement) setPlace(m.placement);
          setMode('idle');
          setP(0);
          break;
        case 'hold':
          setMode('hold');
          setP(m.p);
          setV(m.v);
          break;
        case 'cancel':
          setMode('cancel');
          timer.current = window.setTimeout(() => setMode('idle'), 260);
          break;
        case 'commit':
          setMode('commit');
          setP(1);
          setV(m.v);
          timer.current = window.setTimeout(() => setMode('idle'), 1800);
          break;
        default:
          setMode('idle');
          setP(0);
      }
    }).then((u) => (cancelled ? u() : (unlisten = u)));
    return () => {
      cancelled = true;
      unlisten?.();
      window.clearTimeout(timer.current);
    };
  }, []);

  if (!place || mode === 'idle') return <div className="xf-root" />;
  const anim = { hold: 'xf-grow', cancel: 'xf-shrink', commit: 'xf-commit' }[mode];
  return (
    <div className="xf-root">
      <Bubble
        side={place.side}
        v={v}
        w={place.w}
        h={place.h}
        p={p}
        anim={anim}
        ripples={mode === 'commit' ? 'send' : undefined}
        center={mode === 'commit' ? <Check /> : <Arrow />}
      />
    </div>
  );
}

/**
 * Janela do PC que recebe. Mostra o anel de andamento (animação de chegada, 0,9 s depois da
 * saída no outro PC), que se transforma numa bandeja de onde os arquivos recebidos podem ser
 * arrastados para qualquer lugar. Pegar o último arquivo fecha a bandeja.
 */
export function XferRecv() {
  useTransparentBody();
  const [st, dispatch] = useReducer(recvReducer, initialRecv);
  const [place, setPlace] = useState<XferPlacement | null>(null);
  const [flash, setFlash] = useState(false);
  const ringShownAt = useRef(0);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen<XferRecvEvent>('xfer-recv', (e) => {
      if (e.payload.placement) setPlace(e.payload.placement);
      dispatch({ t: 'file', f: e.payload.file });
    }).then((u) => (cancelled ? u() : (unlisten = u)));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // A chegada só começa depois da animação de saída no outro PC.
  useEffect(() => {
    if (st.phase !== 'wait') return;
    const t = window.setTimeout(() => {
      ringShownAt.current = performance.now();
      dispatch({ t: 'show' });
    }, ARRIVE_DELAY_MS);
    return () => window.clearTimeout(t);
  }, [st.phase]);

  // Anel completo -> pisca e vira bandeja (com um tempo mínimo na tela para arquivos pequenos).
  useEffect(() => {
    if (st.phase !== 'ring' || !st.finished) return;
    const wait = Math.max(0, ringShownAt.current + MIN_RING_MS - performance.now());
    let t2: number | undefined;
    const t1 = window.setTimeout(() => {
      setFlash(true);
      t2 = window.setTimeout(() => {
        setFlash(false);
        dispatch({ t: 'tray' });
      }, 800);
    }, wait);
    return () => {
      window.clearTimeout(t1);
      window.clearTimeout(t2);
    };
  }, [st.phase, st.finished]);

  const rows = st.items.length + (st.active ? 1 : 0);
  useEffect(() => {
    if (st.phase !== 'tray') return;
    const h = 26 + Math.min(MAX_ROWS, Math.max(1, rows)) * ROW_H;
    void xfer.layout('xfer-recv', TRAY_W, h, true).then((pl) => pl && setPlace(pl));
  }, [st.phase, rows]);

  useEffect(() => {
    if (st.phase === 'hidden') void xfer.hide('xfer-recv');
  }, [st.phase]);

  const take = async (f: TrayFile) => {
    if (await xfer.drag(f.id)) dispatch({ t: 'remove', id: f.id });
  };
  const dismiss = () => {
    dispatch({ t: 'dismiss' });
    void xfer.dismiss();
  };

  const side = place?.side ?? 'right';
  if (st.phase === 'ring' && place) {
    const pct = Math.round((flash || st.finished ? 1 : (st.active?.p ?? 0)) * 100);
    return (
      <div className="xf-root">
        <Bubble
          side={side}
          v={place.v}
          w={place.w}
          h={place.h}
          p={flash ? 1 : (st.active?.p ?? 1)}
          anim={flash ? 'xf-done' : 'xf-arrive'}
          ripples={flash ? 'done' : 'arrive'}
          center={flash ? <Check /> : <span className="xf-pct">{pct}%</span>}
        />
      </div>
    );
  }
  if (st.phase === 'tray') {
    return (
      <div className="xf-root">
        <div className={`xf-tray xf-side-${side}`}>
          <button
            className="xf-x"
            onClick={dismiss}
            title="Fechar (os arquivos ficam em Downloads)"
          >
            ✕
          </button>
          <div className="xf-rows" style={{ maxHeight: MAX_ROWS * ROW_H }}>
            {st.active && (
              <div className="xf-row xf-busy">
                <FileGlyph />
                <div className="xf-meta">
                  <div className="xf-name">{st.active.name}</div>
                  <div className="xf-bar">
                    <i style={{ width: `${Math.round(st.active.p * 100)}%` }} />
                  </div>
                </div>
              </div>
            )}
            {st.items.map((f) => (
              <DragRow key={f.id} file={f} onDrag={() => void take(f)} />
            ))}
          </div>
        </div>
      </div>
    );
  }
  return <div className="xf-root" />;
}

const FileGlyph = () => (
  <svg viewBox="0 0 24 24" width={30} height={30} className="xf-glyph">
    <path d="M6 2.5h8l5 5V21a.5.5 0 0 1-.5.5h-12A.5.5 0 0 1 6 21z" fill="#e9eef5" />
    <path d="M14 2.5v5h5" fill="#b7c3d3" />
  </svg>
);

/** Linha arrastável: ao mover o mouse com o botão pressionado, inicia o arrastar nativo (OLE). */
function DragRow({ file, onDrag }: { file: TrayFile; onDrag: () => void }) {
  const start = useRef<{ x: number; y: number } | null>(null);
  useEffect(() => {
    const move = (e: MouseEvent) => {
      const s = start.current;
      if (!s) return;
      if ((e.buttons & 1) === 0) {
        start.current = null;
      } else if (Math.hypot(e.clientX - s.x, e.clientY - s.y) > 6) {
        start.current = null;
        onDrag();
      }
    };
    const up = () => (start.current = null);
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', up);
    return () => {
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', up);
    };
  }, [onDrag]);
  return (
    <div
      className="xf-row xf-file"
      onMouseDown={(e) => {
        if (e.button === 0) start.current = { x: e.clientX, y: e.clientY };
      }}
      title={file.path}
    >
      <FileGlyph />
      <div className="xf-meta">
        <div className="xf-name">{file.name}</div>
        <div className="xf-size">{formatBytes(file.size)}</div>
      </div>
    </div>
  );
}
