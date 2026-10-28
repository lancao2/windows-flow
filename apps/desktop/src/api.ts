import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  AppSettings,
  ConfirmRequest,
  DeviceInfo,
  DiscStatus,
  EngineStatus,
  PeerInfo,
  Side,
  XferPlacement,
} from '@windows-flow/shared-types';

export const api = {
  info: () => invoke<DeviceInfo>('get_info'),
  status: () => invoke<EngineStatus | null>('get_status'),
  host: (port?: number) => invoke<void>('host', { port }),
  connect: (addr: string) => invoke<void>('connect', { addr }),
  confirm: (accept: boolean, remember = false) => invoke<void>('confirm', { accept, remember }),
  disconnect: () => invoke<void>('disconnect'),
  setPeerSide: (side: Side) => invoke<void>('set_peer_side', { side }),
  setPortalEnabled: (enabled: boolean) => invoke<void>('set_portal_enabled', { enabled }),
  previewPortal: () => invoke<void>('preview_portal'),
  settings: () => invoke<AppSettings>('get_settings'),
  peers: () => invoke<PeerInfo[]>('get_peers'),
  setVisible: (visible: boolean) => invoke<void>('set_visible', { visible }),
  setAutoConnect: (enabled: boolean) => invoke<void>('set_auto_connect', { enabled }),
  forgetLastPeer: () => invoke<void>('forget_last_peer'),
  connectPeer: (ip: string, port?: number, remember = false) =>
    invoke<void>('connect_peer', { ip, port, remember }),
  forgetKnown: (fingerprint: string) => invoke<void>('forget_known', { fingerprint }),
  discovery: () => invoke<DiscStatus | null>('get_discovery'),
  toastDismiss: (id: string) => invoke<void>('toast_dismiss', { id }),
};

/** Comandos das janelas de transferência (anel de envio e bandeja de recebimento). */
export const xfer = {
  layout: (label: 'xfer-send' | 'xfer-recv', w: number, h: number, interactive: boolean) =>
    invoke<XferPlacement | null>('xfer_layout', { label, w, h, interactive }),
  hide: (label: 'xfer-send' | 'xfer-recv') => invoke<void>('xfer_hide', { label }),
  dismiss: () => invoke<void>('xfer_dismiss'),
  /** Começa o arrastar nativo; resolve quando o usuário solta. `true` = algum destino aceitou. */
  drag: (id: number) => invoke<boolean>('xfer_drag', { id }),
  /** Duplo clique: abre o arquivo/pasta com o programa padrão. */
  open: (id: number) => invoke<boolean>('xfer_open', { id }),
  /** Abre o Explorer com o item selecionado. */
  reveal: (id: number) => invoke<boolean>('xfer_reveal', { id }),
};

export function subscribe(handlers: {
  onStatus: (s: EngineStatus) => void;
  onConfirm: (c: ConfirmRequest) => void;
  onNotice: (m: string) => void;
  onSettings: (s: AppSettings) => void;
  onPeers: (p: PeerInfo[]) => void;
  onDiscovery: (d: DiscStatus) => void;
}): Promise<UnlistenFn[]> {
  return Promise.all([
    listen<EngineStatus>('engine-status', (e) => handlers.onStatus(e.payload)),
    listen<ConfirmRequest>('engine-confirm', (e) => handlers.onConfirm(e.payload)),
    listen<string>('engine-notice', (e) => handlers.onNotice(e.payload)),
    listen<AppSettings>('settings-changed', (e) => handlers.onSettings(e.payload)),
    listen<PeerInfo[]>('peers-changed', (e) => handlers.onPeers(e.payload)),
    listen<DiscStatus>('discovery-status', (e) => handlers.onDiscovery(e.payload)),
  ]);
}
