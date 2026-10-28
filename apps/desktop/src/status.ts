import type { ControlState, LinkState, Side } from '@windows-flow/shared-types';

const STATE_LABELS: Record<ControlState, string> = {
  local: 'Controlando este PC',
  armed: 'Na borda da tela',
  remote: 'Controlando o outro PC',
  recovering: 'Recuperando controle local',
};

const LINK_LABELS: Record<LinkState, string> = {
  idle: 'Sem conexão',
  listening: 'Aguardando o outro PC',
  connecting: 'Conectando…',
  awaiting_confirm: 'Aguardando confirmação do código',
  handshaking: 'Estabelecendo sessão…',
  connected: 'Conectado',
};

const SIDE_LABELS: Record<Side, string> = {
  left: 'à esquerda',
  right: 'à direita',
  top: 'acima',
  bottom: 'abaixo',
};

export const stateLabel = (s: ControlState): string => STATE_LABELS[s];
export const linkLabel = (s: LinkState): string => LINK_LABELS[s];
export const sideLabel = (s: Side): string => SIDE_LABELS[s];

/** Aceita "192.168.0.10" ou "192.168.0.10:47800". */
export function looksLikePeerAddress(input: string): boolean {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})(?::(\d{1,5}))?$/.exec(input.trim());
  if (!m) return false;
  const octets = m.slice(1, 5).map(Number);
  const port = m[5] === undefined ? 1 : Number(m[5]);
  return octets.every((o) => o <= 255) && port >= 1 && port <= 65535;
}

/** Lado (em relação ao centro) mais próximo de um deslocamento: o eixo dominante decide. */
export function nearestSide(dx: number, dy: number): Side {
  if (Math.abs(dx) * 0.7 >= Math.abs(dy)) return dx >= 0 ? 'right' : 'left';
  return dy >= 0 ? 'bottom' : 'top';
}
