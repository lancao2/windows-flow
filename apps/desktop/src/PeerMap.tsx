import { useRef, useState, type PointerEvent } from 'react';
import type { Side } from '@windows-flow/shared-types';
import { nearestSide, sideLabel } from './status';

const STAGE_W = 400;
const STAGE_H = 230;
/** Centro de cada encaixe em relação ao centro do palco. */
const SLOT: Record<Side, [number, number]> = {
  right: [140, 0],
  left: [-140, 0],
  top: [0, -82],
  bottom: [0, 82],
};

interface Props {
  side: Side;
  deviceName: string;
  connected: boolean;
  /** Bloqueado enquanto um PC controla o outro. */
  locked: boolean;
  onChange: (side: Side) => void;
}

/**
 * Mapa dos dois PCs: arraste o "outro PC" para o lado onde ele está de verdade. Usa eventos de
 * ponteiro (o arrastar HTML5 não funciona nesta janela, que trata soltar arquivos).
 */
export function PeerMap({ side, deviceName, connected, locked, onChange }: Props) {
  const stage = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<[number, number] | null>(null);

  const rel = (e: PointerEvent): [number, number] => {
    const r = stage.current?.getBoundingClientRect();
    if (!r) return [0, 0];
    return [e.clientX - (r.left + r.width / 2), e.clientY - (r.top + r.height / 2)];
  };
  const hovered = drag ? nearestSide(drag[0], drag[1]) : null;

  const [px, py] = drag ?? SLOT[side];
  return (
    <div
      ref={stage}
      className={`pm-stage${locked ? ' pm-locked' : ''}`}
      style={{ width: STAGE_W, height: STAGE_H }}
    >
      {(Object.keys(SLOT) as Side[]).map((s) => (
        <button
          key={s}
          type="button"
          disabled={locked}
          className={`pm-slot pm-slot-${s}${hovered === s ? ' pm-hot' : ''}${side === s && !drag ? ' pm-current' : ''}`}
          style={{ transform: `translate(${SLOT[s][0]}px, ${SLOT[s][1]}px)` }}
          onClick={() => onChange(s)}
          aria-label={`Outro PC ${sideLabel(s)}`}
        />
      ))}

      <div className={`pm-tile pm-me pm-edge-${side}`}>
        <div className="pm-screen" />
        <b>Este PC</b>
        <span>{deviceName}</span>
      </div>

      <div
        className={`pm-tile pm-peer${drag ? ' pm-dragging' : ''}${connected ? ' pm-on' : ''}`}
        style={{ transform: `translate(${px}px, ${py}px)` }}
        onPointerDown={(e) => {
          if (locked) return;
          e.preventDefault();
          e.currentTarget.setPointerCapture(e.pointerId);
          setDrag(rel(e));
        }}
        onPointerMove={(e) => drag && setDrag(rel(e))}
        onPointerUp={(e) => {
          if (!drag) return;
          const [dx, dy] = rel(e);
          setDrag(null);
          const next = nearestSide(dx, dy);
          if (next !== side) onChange(next);
        }}
        onPointerCancel={() => setDrag(null)}
        role="button"
        aria-label="Arraste para o lado onde o outro PC está"
      >
        <div className="pm-screen" />
        <b>Outro PC</b>
        <span>{connected ? 'conectado' : 'arraste-me'}</span>
      </div>
    </div>
  );
}
