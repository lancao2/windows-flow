import { useEffect, useState } from 'react';
import type { ConfirmRequest, DeviceInfo, EngineStatus, Side } from '@windows-flow/shared-types';
import { api, subscribe } from './api';
import { linkLabel, looksLikePeerAddress, sideLabel, stateLabel } from './status';

const SIDES: Side[] = ['left', 'right', 'top', 'bottom'];

export function App() {
  const [info, setInfo] = useState<DeviceInfo | null>(null);
  const [status, setStatus] = useState<EngineStatus | null>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [notices, setNotices] = useState<string[]>([]);
  const [peerInput, setPeerInput] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [portalOn, setPortalOn] = useState(() => {
    try {
      return localStorage.getItem('portal') !== 'off';
    } catch {
      return true;
    }
  });

  useEffect(() => {
    void api.setPortalEnabled(portalOn);
    try {
      localStorage.setItem('portal', portalOn ? 'on' : 'off');
    } catch {
      /* sem armazenamento: usa o padrão */
    }
  }, [portalOn]);

  useEffect(() => {
    let unlisten: Array<() => void> = [];
    let cancelled = false;
    subscribe({
      onStatus: (s) => {
        setStatus(s);
        if (s.link !== 'awaiting_confirm') setConfirm(null);
      },
      onConfirm: setConfirm,
      onNotice: (m) =>
        setNotices((n) => [`${new Date().toLocaleTimeString()}  ${m}`, ...n].slice(0, 30)),
    }).then((u) => (cancelled ? u.forEach((f) => f()) : (unlisten = u)));
    void api.info().then(setInfo);
    void api.status().then((s) => s && setStatus(s));
    return () => {
      cancelled = true;
      unlisten.forEach((f) => f());
    };
  }, []);

  const connected = status?.link === 'connected';
  const active = status !== null && status.link !== 'idle';
  const beingControlled = status?.controlled ?? false;
  const controlling = status?.state === 'remote';

  const doConnect = () => {
    if (!looksLikePeerAddress(peerInput)) {
      setError('Digite o IP do outro PC, por exemplo 192.168.0.10');
      return;
    }
    setError(null);
    void api.connect(peerInput.trim());
  };

  return (
    <main className="app">
      <header>
        <h1>Windows Flow</h1>
        <span className="muted">{info?.device_name}</span>
      </header>

      {(beingControlled || controlling) && (
        <section className="banner" role="alert">
          <strong>
            {beingControlled
              ? 'Este PC está sendo controlado pelo outro computador.'
              : 'Você está controlando o outro computador.'}
          </strong>
          <button className="danger" onClick={() => void api.disconnect()}>
            Desconectar
          </button>
        </section>
      )}

      <section className="card">
        <h2>Estado</h2>
        <p>
          <b>{status ? stateLabel(status.state) : '…'}</b> · {status ? linkLabel(status.link) : '…'}
          {status?.peer && <span className="muted"> · par {status.peer}</span>}
        </p>
        {active && !beingControlled && !controlling && (
          <button onClick={() => void api.disconnect()}>Desconectar</button>
        )}
      </section>

      {!active && (
        <section className="card">
          <h2>Conectar dois PCs</h2>
          <ol className="steps">
            <li>
              <b>PC 1:</b> clique em “Aguardar conexão neste PC” e anote o IP abaixo.
            </li>
            <li>
              <b>PC 2:</b> digite o IP do PC 1 e clique em “Conectar”.
            </li>
            <li>Nos dois PCs, confira se o código é igual e confirme.</li>
            <li>Escolha onde está o outro PC (esquerda, direita…) em cada um.</li>
          </ol>
          <div className="row">
            <button className="primary" onClick={() => void api.host()}>
              Aguardar conexão neste PC
            </button>
            <span className="muted">
              IP deste PC: <span className="big-ip">{info?.local_ip ?? 'desconhecido'}</span> ·
              porta {info?.default_port}
            </span>
          </div>
          <div className="row">
            <input
              placeholder="IP do outro PC (ex.: 192.168.0.10)"
              value={peerInput}
              onChange={(e) => setPeerInput(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && doConnect()}
            />
            <button onClick={doConnect}>Conectar</button>
          </div>
          {error && <p className="error">{error}</p>}
        </section>
      )}

      {status?.link === 'listening' && (
        <section className="card">
          Aguardando o outro PC em <b>{status.listening ?? '…'}</b>. Se o Windows pedir acesso ao
          Firewall, permita apenas em <b>redes privadas</b>.
        </section>
      )}

      <section className="card">
        <h2>Posição do outro PC</h2>
        <p className="muted">
          Cada PC informa onde o <b>outro</b> está. Ex.: se o outro PC fica à direita deste, escolha
          “direita” aqui e “esquerda” no outro.
        </p>
        <div className="row">
          {SIDES.map((s) => (
            <button
              key={s}
              className={status?.peer_side === s ? 'selected' : ''}
              disabled={beingControlled || controlling}
              onClick={() => void api.setPeerSide(s)}
            >
              {sideLabel(s)}
            </button>
          ))}
        </div>
      </section>

      <section className="card">
        <h2>Efeito de transição</h2>
        <div className="row">
          <label>
            <input
              type="checkbox"
              checked={portalOn}
              onChange={(e) => setPortalOn(e.target.checked)}
            />{' '}
            Mostrar animação ao cruzar a borda
          </label>
          <button
            disabled={!portalOn}
            onClick={() => {
              void api.previewPortal();
              // O overlay informa o estado ao tocar; atualiza o texto logo depois.
              setTimeout(() => void api.info().then(setInfo), 700);
            }}
          >
            Ver animação
          </button>
        </div>
        <p className="muted">Estado da animação: {info?.portal_health ?? '…'}</p>
      </section>

      <section className="card">
        <h2>Segurança</h2>
        <p>
          Atalho de emergência: <b>Ctrl + Alt + Shift + F12</b> — devolve mouse e teclado a este PC
          na hora, sem depender da rede.
          {info && !info.hotkey_registered && (
            <span className="error">
              {' '}
              (Outro programa já usa este atalho; ele ainda funciona durante o controle remoto.)
            </span>
          )}
        </p>
        {connected && <p className="muted">Teclas digitadas nunca são gravadas nem registradas.</p>}
      </section>

      {notices.length > 0 && (
        <section className="card">
          <h2>Eventos</h2>
          <ul className="log">
            {notices.map((n, i) => (
              <li key={i}>{n}</li>
            ))}
          </ul>
        </section>
      )}

      {confirm && (
        <div className="modal" role="dialog" aria-modal="true">
          <div className="dialog">
            <h2>Confirmar pareamento</h2>
            <p>
              Um PC em <b>{confirm.peer}</b> quer se conectar. Confira se o código abaixo é{' '}
              <b>exatamente igual</b> ao exibido no outro PC.
            </p>
            <div className="sas">{confirm.sas}</div>
            <div className="row">
              <button className="primary" onClick={() => void api.confirm(true)}>
                Os códigos são iguais
              </button>
              <button className="danger" onClick={() => void api.confirm(false)}>
                Não são iguais
              </button>
            </div>
          </div>
        </div>
      )}
    </main>
  );
}
