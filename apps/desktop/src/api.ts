import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  ConfirmRequest,
  DeviceInfo,
  EngineStatus,
  Side,
  XferPlacement,
} from '@windows-flow/shared-types';

export const api = {
  info: () => invoke<DeviceInfo>('get_info'),
  status: () => invoke<EngineStatus | null>('get_status'),
  host: (port?: number) => invoke<void>('host', { port }),
  connect: (addr: string) => invoke<void>('connect', { addr }),
  confirm: (accept: boolean) => invoke<void>('confirm', { accept }),
  disconnect: () => invoke<void>('disconnect'),
  setPeerSide: (side: Side) => invoke<void>('set_peer_side', { side }),
  setPortalEnabled: (enabled: boolean) => invoke<void>('set_portal_enabled', { enabled }),
  previewPortal: () => invoke<void>('preview_portal'),
};

/** Comandos das janelas de transferência (anel de envio e bandeja de recebimento). */
export const xfer = {
  layout: (label: 'xfer-send' | 'xfer-recv', w: number, h: number, interactive: boolean) =>
    invoke<XferPlacement | null>('xfer_layout', { label, w, h, interactive }),
  hide: (label: 'xfer-send' | 'xfer-recv') => invoke<void>('xfer_hide', { label }),
  dismiss: () => invoke<void>('xfer_dismiss'),
  /** Começa o arrastar nativo; resolve quando o usuário solta. `true` = algum destino aceitou. */
  drag: (id: number) => invoke<boolean>('xfer_drag', { id }),
};

export function subscribe(handlers: {
  onStatus: (s: EngineStatus) => void;
  onConfirm: (c: ConfirmRequest) => void;
  onNotice: (m: string) => void;
}): Promise<UnlistenFn[]> {
  return Promise.all([
    listen<EngineStatus>('engine-status', (e) => handlers.onStatus(e.payload)),
    listen<ConfirmRequest>('engine-confirm', (e) => handlers.onConfirm(e.payload)),
    listen<string>('engine-notice', (e) => handlers.onNotice(e.payload)),
  ]);
}
