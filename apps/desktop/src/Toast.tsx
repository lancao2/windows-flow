import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import type { ToastEvent } from '@windows-flow/shared-types';
import { api } from './api';

const AUTO_HIDE_MS = 15_000;

/** Notificação "PC encontrado na rede" com conexão em um clique (sem digitar IP). */
export function Toast() {
  const [t, setT] = useState<ToastEvent | null>(null);
  const [always, setAlways] = useState(false);

  useEffect(() => {
    document.body.classList.add('xfer-body');
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen<ToastEvent>('toast', (e) => {
      setAlways(false);
      setT(e.payload);
    }).then((u) => (cancelled ? u() : (unlisten = u)));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!t) return;
    const timer = window.setTimeout(() => void api.toastDismiss(t.id), AUTO_HIDE_MS);
    return () => window.clearTimeout(timer);
  }, [t]);

  if (!t) return <div className="xf-root" />;
  return (
    <div className="xf-root">
      <div className="toast" key={t.id}>
        <div className="toast-icon" aria-hidden>
          <i />
        </div>
        <div className="toast-body">
          <b>{t.name}</b>
          <span>quer conectar? Está na sua rede ({t.ip})</span>
          <label className="toast-check">
            <input type="checkbox" checked={always} onChange={(e) => setAlways(e.target.checked)} />{' '}
            Não perguntar de novo (conectar sempre)
          </label>
          <div className="toast-actions">
            <button className="primary" onClick={() => void api.connectPeer(t.ip, t.port, always)}>
              Conectar
            </button>
            <button onClick={() => void api.toastDismiss(t.id)}>Agora não</button>
          </div>
        </div>
      </div>
    </div>
  );
}
