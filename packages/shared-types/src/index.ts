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
  local_ip: string | null;
  default_port: number;
  hotkey_registered: boolean;
  /** Estado da animação de transição informado pelo overlay (WebGL ok, plano B, erro...). */
  portal_health: string;
}

export const PROTOCOL_VERSION = 1;
