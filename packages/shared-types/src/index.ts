/** Estados da máquina de continuidade (espelha crates/continuity-core). */
export type ControlState = 'local' | 'armed' | 'remote' | 'recovering';

export type LinkState =
  'idle' | 'listening' | 'connecting' | 'awaiting_confirm' | 'handshaking' | 'connected';

export type Side = 'left' | 'right' | 'top' | 'bottom';

/** Espelha continuity_core::engine::Status. */
export interface EngineStatus {
  state: ControlState;
  link: LinkState;
  /** Este PC está sendo controlado pelo outro. */
  controlled: boolean;
  peer_side: Side;
  peer: string | null;
  listening: string | null;
}

export interface ConfirmRequest {
  sas: string;
  peer: string;
  /** Nome do PC, se ele foi visto na rede. */
  name: string | null;
  /** Marcar "lembrar este computador" por padrão. */
  remember: boolean;
}

/** Evento do overlay de transição (janela transparente junto à borda com a tela distorcida). */
export interface PortalEvent {
  side: Side;
  /** true: cursor chegando a este PC (ondas se expandem); false: saindo (ondas se contraem). */
  enter: boolean;
  /** Tamanho da janela/captura em pixels físicos. */
  w: number;
  h: number;
  /** Posição do ponto de contato ao longo da borda, em px dentro da janela. */
  v0: number;
}
export interface DeviceInfo {
  device_name: string;
  /** Versão do app (os dois PCs precisam estar na mesma). */
  version: string;
  local_ip: string | null;
  /** Outros endereços da LAN deste PC (placas virtuais/VPN). */
  other_ips: string[];
  default_port: number;
  hotkey_registered: boolean;
  /** Estado da animação de transição informado pelo overlay (WebGL ok, plano B, erro...). */
  portal_health: string;
}

export const PROTOCOL_VERSION = 4;

/** Posição da janela de transferência rente à borda (espelha xfer::Placement). */
export interface XferPlacement {
  side: Side;
  /** Ponto de contato ao longo da borda, em px CSS dentro da janela. */
  v: number;
  w: number;
  h: number;
}

/** Evento da janela de envio (captura do arrastar). */
export interface XferSendEvent {
  kind: 'catch' | 'hold' | 'cancel' | 'commit' | 'idle';
  /** Progresso do anel, 0..1. */
  p: number;
  placement: XferPlacement | null;
  /** Posição do cursor ao longo da borda, em px CSS dentro da janela. */
  v: number;
}

/** Espelha continuity_core::engine::FileEvent. */
export interface FileEvent {
  dir: 'in' | 'out';
  state: 'offer' | 'progress' | 'done' | 'failed';
  id: number;
  name: string;
  size: number;
  index: number;
  count: number;
  batch_done: number;
  batch_bytes: number;
  ratio: number;
  side: Side;
  screen: [number, number, number, number];
  path: string | null;
  /** Item raiz: o arquivo avulso ou a pasta inteira. */
  root: string;
  root_path: string | null;
  root_bytes: number;
  /** Último arquivo do item raiz (só em `done`). */
  root_last: boolean;
  is_dir: boolean;
}

export interface XferRecvEvent {
  file: FileEvent;
  placement: XferPlacement | null;
}

/** PC com o app visto na rede (espelha discovery::Peer). */
export interface PeerInfo {
  id: string;
  name: string;
  ip: string;
  port: number;
}

/** Computador confiável (espelha auto::KnownPc). */
export interface KnownPc {
  name: string;
  fingerprint: string;
  ip: string | null;
  /** Lado em que ele fica em relação a este PC. */
  side: Side | null;
}

/** Diagnóstico da descoberta na rede. */
export interface DiscStatus {
  listening: boolean;
  interfaces: number;
  received: number;
}

/** Preferências salvas (espelha auto::Settings). */
export interface AppSettings {
  /** IP do último PC conectado. */
  last_peer: string | null;
  /** Ficar visível na rede e aguardar conexões sozinho. */
  visible: boolean;
  /** Reconectar sozinho aos computadores conhecidos. */
  auto_connect: boolean;
  known: KnownPc[];
}

/** Notificação "PC encontrado na rede". */
export interface ToastEvent {
  id: string;
  name: string;
  ip: string;
  port: number;
}
