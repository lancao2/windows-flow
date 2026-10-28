import { useEffect, useMemo, useState, type ReactNode } from 'react';
import type {
  AppSettings,
  ConfirmRequest,
  DeviceInfo,
  DiscStatus,
  EngineStatus,
  PeerInfo,
} from '@windows-flow/shared-types';
import { api, subscribe } from './api';
import {
  IconArrowRight,
  IconBolt,
  IconCheck,
  IconClock,
  IconCopy,
  IconDashboard,
  IconFile,
  IconInfo,
  IconKeyboard,
  IconLink,
  IconMonitor,
  IconPower,
  IconRadar,
  IconSend,
  IconShield,
  IconSliders,
  IconSparkles,
  IconSwap,
  IconTrash,
  IconWifi,
  Logo,
} from './Icons';
import { PeerMap } from './PeerMap';
import { linkLabel, looksLikePeerAddress, sideLabel } from './status';

type Tab = 'home' | 'connect' | 'settings';
type Mode = 'host' | 'join';
interface Note {
  time: string;
  msg: string;
}

const noteTone = (m: string): 'ok' | 'bad' | 'info' =>
  /falha|não foi|erro|recus|cancel|esgotad|diferente/i.test(m)
    ? 'bad'
    : /conectad|enviad|salv|restaurad/i.test(m)
      ? 'ok'
      : 'info';

function Switch({
  checked,
  onChange,
  label,
  hint,
  icon,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  hint?: string;
  icon: ReactNode;
}) {
  return (
    <label className="row-item switch-row">
      <span className="ri-icon">{icon}</span>
      <span className="ri-text">
        <b>{label}</b>
        {hint && <small>{hint}</small>}
      </span>
      <input
        type="checkbox"
        className="switch"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
    </label>
  );
}

export function App() {
  const [tab, setTab] = useState<Tab>('home');
  const [info, setInfo] = useState<DeviceInfo | null>(null);
  const [status, setStatus] = useState<EngineStatus | null>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [notes, setNotes] = useState<Note[]>([]);
  const [peerInput, setPeerInput] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [mode, setMode] = useState<Mode>('host');
  const [copied, setCopied] = useState(false);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [peers, setPeers] = useState<PeerInfo[]>([]);
  const [disc, setDisc] = useState<DiscStatus | null>(null);
  const [remember, setRemember] = useState(false);
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
      onConfirm: (c) => {
        setRemember(c.remember);
        setConfirm(c);
      },
      onSettings: setSettings,
      onPeers: setPeers,
      onDiscovery: setDisc,
      onNotice: (m) =>
        setNotes((n) =>
          [{ time: new Date().toLocaleTimeString().slice(0, 5), msg: m }, ...n].slice(0, 30),
        ),
    }).then((u) => (cancelled ? u.forEach((f) => f()) : (unlisten = u)));
    void api.info().then(setInfo);
    void api.settings().then(setSettings);
    void api.peers().then(setPeers);
    void api.discovery().then(setDisc);
    void api.status().then((s) => s && setStatus(s));
    return () => {
      cancelled = true;
      unlisten.forEach((f) => f());
    };
  }, []);

  const link = status?.link ?? 'idle';
  const connected = link === 'connected';
  const canStart = link === 'idle' || link === 'listening';
  const beingControlled = status?.controlled ?? false;
  const controlling = status?.state === 'remote';
  const tone = connected ? 'ok' : link === 'idle' ? 'off' : 'wait';
  const lastPeer = settings?.last_peer ?? null;
  const peerIp = status?.peer?.split(':')[0] ?? null;
  const peerName = useMemo(() => {
    if (!peerIp) return null;
    return (
      peers.find((p) => p.ip === peerIp)?.name ??
      settings?.known.find((k) => k.ip === peerIp)?.name ??
      null
    );
  }, [peerIp, peers, settings]);
  const side = status?.peer_side ?? 'right';

  const doConnect = () => {
    if (!looksLikePeerAddress(peerInput)) {
      setError('Digite o IP do outro PC, por exemplo 192.168.0.10');
      return;
    }
    setError(null);
    void api.connect(peerInput.trim());
  };

  const copyIp = () => {
    if (!info?.local_ip) return;
    void navigator.clipboard
      ?.writeText(info.local_ip)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      })
      .catch(() => undefined);
  };

  /* ---- peças reutilizadas ------------------------------------------------------------ */

  const peerList = (max: number) => (
    <div className="plist">
      {peers.slice(0, max).map((p) => {
        const here = connected && peerIp === p.ip;
        return (
          <div className="row-item" key={p.id}>
            <span className="ri-icon grad">
              <IconMonitor />
            </span>
            <span className="ri-text">
              <b>{p.name}</b>
              <small>{p.ip}</small>
            </span>
            {here ? (
              <span className="chip ok">Conectado</span>
            ) : (
              <button
                className="btn mini primary"
                disabled={!canStart}
                onClick={() => void api.connectPeer(p.ip, p.port)}
              >
                Conectar
              </button>
            )}
          </div>
        );
      })}
      {lastPeer && !peers.some((p) => p.ip === lastPeer) && (
        <div className="row-item dim">
          <span className="ri-icon">
            <IconClock />
          </span>
          <span className="ri-text">
            <b>Último PC</b>
            <small>{lastPeer} · fora da rede agora</small>
          </span>
          <button
            className="btn mini"
            disabled={!canStart}
            onClick={() => void api.connectPeer(lastPeer)}
          >
            Tentar
          </button>
        </div>
      )}
      {peers.length === 0 && !lastPeer && (
        <div className="empty">
          <span className="radar">
            <IconRadar size={34} />
          </span>
          <b>{settings?.visible === false ? 'Visibilidade desligada' : 'Procurando…'}</b>
          <small>
            {settings?.visible === false
              ? 'Ative em Ajustes para encontrar outros PCs.'
              : 'Abra o Windows Flow no outro PC, na mesma rede.'}
          </small>
        </div>
      )}
    </div>
  );

  /* ---- páginas ------------------------------------------------------------------------ */

  const heroTitle = connected
    ? 'Conectado'
    : link === 'listening'
      ? 'Visível na rede'
      : link === 'idle'
        ? 'Sem conexão'
        : linkLabel(link);
  const heroSub = connected
    ? `${peerName ?? peerIp ?? 'outro PC'}${beingControlled ? ' · controlando este PC' : controlling ? ' · você controla o outro' : ''}`
    : link === 'listening'
      ? `Aguardando em ${info?.local_ip ?? '…'}:${info?.default_port ?? ''}`
      : 'Conecte este PC a outro computador da rede';

  const hero = (
    <section className="card hero">
      <svg className="hero-art" viewBox="0 0 300 160" aria-hidden>
        <path
          d="M120 160c20-50 70-60 100-110 20-30 50-40 80-50v160z"
          fill="rgba(255,255,255,.16)"
        />
        <path d="M170 160c10-40 50-50 70-80 15-22 35-30 60-36v116z" fill="rgba(255,255,255,.14)" />
      </svg>
      <div className="hero-top">
        <span className="hero-label">
          <IconLink size={16} /> Conexão
        </span>
        <span className="hero-chip">{sideLabel(side)}</span>
      </div>
      <div className="hero-title">{heroTitle}</div>
      <div className="hero-sub">{heroSub}</div>
      <div className="hero-actions">
        {connected || !canStart ? (
          <button className="btn dark" onClick={() => void api.disconnect()}>
            <IconPower size={16} /> {connected ? 'Desconectar' : 'Cancelar'}
          </button>
        ) : (
          <button className="btn dark" onClick={() => setTab('connect')}>
            Conectar <IconArrowRight size={16} />
          </button>
        )}
        <button className="btn light" onClick={copyIp} disabled={!info?.local_ip}>
          {copied ? <IconCheck size={16} /> : <IconCopy size={16} />}{' '}
          {copied ? 'Copiado' : 'Copiar meu IP'}
        </button>
      </div>
    </section>
  );

  const statCards = (
    <>
      <section className="card stat">
        <div>
          <span className="stat-label">Este PC</span>
          <b className="stat-value mono">{info?.local_ip ?? '—'}</b>
          <small>{info?.device_name}</small>
        </div>
        <span className="stat-ic blue">
          <IconMonitor />
        </span>
      </section>
      <section className="card stat">
        <div>
          <span className="stat-label">Na rede</span>
          <b className="stat-value">
            {peers.length} <small>{peers.length === 1 ? 'computador' : 'computadores'}</small>
          </b>
          <small>{disc?.listening === false ? 'porta de busca ocupada' : 'busca ativa'}</small>
        </div>
        <span className={`chip ${peers.length > 0 ? 'ok' : ''}`}>
          {peers.length > 0 ? `+${peers.length}` : '…'}
        </span>
      </section>
    </>
  );

  const mapCard = (
    <section className="card map">
      <header className="card-h">
        <h2>
          <IconSwap size={18} /> Posição das telas
        </h2>
        <span className="chip">arraste</span>
      </header>
      <PeerMap
        side={side}
        deviceName={info?.device_name ?? 'este PC'}
        connected={connected}
        locked={beingControlled || controlling}
        onChange={(s) => void api.setPeerSide(s)}
      />
      <p className="muted tiny center">
        {beingControlled || controlling
          ? 'Travado enquanto um PC controla o outro.'
          : `O outro PC está ${sideLabel(side)}. Salvo para cada computador conhecido.`}
      </p>
    </section>
  );

  const activityCard = (
    <section className="card activity">
      <header className="card-h">
        <h2>
          <IconBolt size={18} /> Atividade
        </h2>
        <span className="chip">{notes.length}</span>
      </header>
      <div className="alist">
        {notes.slice(0, 6).map((n, i) => (
          <div className="act" key={`${n.time}-${i}`}>
            <span className={`dot ${noteTone(n.msg)}`} />
            <span className="act-msg">{n.msg}</span>
            <time>{n.time}</time>
          </div>
        ))}
        {notes.length === 0 && (
          <div className="empty">
            <span className="radar">
              <IconInfo size={30} />
            </span>
            <small>Os avisos da conexão aparecem aqui.</small>
          </div>
        )}
      </div>
    </section>
  );

  const computersCard = (
    <section className="card computers">
      <header className="card-h">
        <h2>
          <IconWifi size={18} /> Computadores
        </h2>
        <button className="btn mini ghost" onClick={() => setTab('connect')}>
          <IconLink size={14} /> IP
        </button>
      </header>
      {peerList(3)}
    </section>
  );

  const tipsCard = (
    <section className="card tips">
      <header className="card-h">
        <h2>
          <IconSparkles size={18} /> Como usar
        </h2>
      </header>
      <div className="plist">
        <div className="row-item">
          <span className="ri-icon grad">
            <IconSwap />
          </span>
          <span className="ri-text">
            <b>Trocar de PC</b>
            <small>Leve o mouse à borda {sideLabel(side)}</small>
          </span>
        </div>
        <div className="row-item">
          <span className="ri-icon grad2">
            <IconSend />
          </span>
          <span className="ri-text">
            <b>Enviar arquivos</b>
            <small>Segure arquivos ou pastas na borda por 2 s</small>
          </span>
        </div>
        <div className="row-item">
          <span className="ri-icon warn">
            <IconKeyboard />
          </span>
          <span className="ri-text">
            <b>Emergência</b>
            <small>Ctrl + Alt + Shift + F12</small>
          </span>
        </div>
      </div>
    </section>
  );

  const home = (
    <div className="grid home">
      <div className="g-hero">{hero}</div>
      <div className="g-stats">{statCards}</div>
      <div className="g-map">{mapCard}</div>
      <div className="g-act">{activityCard}</div>
      <div className="g-side">
        {computersCard}
        {tipsCard}
      </div>
    </div>
  );

  const connectPage = (
    <div className="grid two">
      <section className="card">
        <header className="card-h">
          <h2>
            <IconLink size={18} /> Conectar por endereço
          </h2>
        </header>
        {canStart ? (
          <>
            <div className="seg">
              <button className={mode === 'host' ? 'on' : ''} onClick={() => setMode('host')}>
                Este PC aguarda
              </button>
              <button className={mode === 'join' ? 'on' : ''} onClick={() => setMode('join')}>
                Conectar a outro PC
              </button>
            </div>
            {mode === 'host' ? (
              <div className="pane">
                <p className="muted">
                  No outro PC, escolha “Conectar a outro PC” e digite este endereço.
                </p>
                <div className="ipbox">
                  <div>
                    <span className="stat-label">Endereço deste PC</span>
                    <div className="big-ip">{info?.local_ip ?? 'desconhecido'}</div>
                    {(info?.other_ips.length ?? 0) > 0 && (
                      <small className="muted">
                        Se não funcionar: {info?.other_ips.join(' · ')}
                      </small>
                    )}
                  </div>
                  <button className="btn" onClick={copyIp} disabled={!info?.local_ip}>
                    {copied ? <IconCheck size={16} /> : <IconCopy size={16} />}{' '}
                    {copied ? 'Copiado' : 'Copiar'}
                  </button>
                </div>
                {link === 'listening' ? (
                  <p className="ok-text">
                    ● Visível na rede, aguardando em {info?.local_ip}:{info?.default_port}
                  </p>
                ) : (
                  <button className="btn primary wide" onClick={() => void api.host()}>
                    Aguardar conexão
                  </button>
                )}
              </div>
            ) : (
              <div className="pane">
                <p className="muted">Digite o endereço que aparece no outro PC.</p>
                <div className="inline">
                  <input
                    placeholder="Ex.: 192.168.0.10"
                    value={peerInput}
                    onChange={(e) => setPeerInput(e.target.value)}
                    onKeyDown={(e) => e.key === 'Enter' && doConnect()}
                  />
                  <button className="btn primary" onClick={doConnect}>
                    Conectar
                  </button>
                </div>
                {error && <p className="error">{error}</p>}
              </div>
            )}
          </>
        ) : (
          <div className="pane">
            <p>{connected ? 'Conectado' : linkLabel(link)}</p>
            {status?.peer && <p className="muted">par {status.peer}</p>}
            <button className="btn" onClick={() => void api.disconnect()}>
              <IconPower size={16} /> {connected ? 'Desconectar' : 'Cancelar'}
            </button>
          </div>
        )}
      </section>
      <section className="card">
        <header className="card-h">
          <h2>
            <IconWifi size={18} /> Encontrados na rede
          </h2>
          <span className="chip">{peers.length}</span>
        </header>
        {peerList(6)}
      </section>
    </div>
  );

  const settingsPage = (
    <div className="grid three">
      <section className="card">
        <header className="card-h">
          <h2>
            <IconSliders size={18} /> Preferências
          </h2>
        </header>
        <div className="plist">
          <Switch
            checked={settings?.visible ?? true}
            onChange={(v) => void api.setVisible(v)}
            icon={<IconWifi />}
            label="Visível na rede"
            hint="Aguardar conexões e ser achado por outros PCs"
          />
          <Switch
            checked={settings?.auto_connect ?? true}
            onChange={(v) => void api.setAutoConnect(v)}
            icon={<IconLink />}
            label="Conectar sozinho"
            hint={
              lastPeer ? `Computadores conhecidos · último ${lastPeer}` : 'Computadores conhecidos'
            }
          />
          <Switch
            checked={portalOn}
            onChange={setPortalOn}
            icon={<IconSparkles />}
            label="Animação ao cruzar a borda"
            hint={info?.portal_health ?? ''}
          />
        </div>
        <button
          className="btn wide"
          disabled={!portalOn}
          onClick={() => {
            void api.previewPortal();
            setTimeout(() => void api.info().then(setInfo), 700);
          }}
        >
          <IconSparkles size={16} /> Ver animação
        </button>
      </section>

      <section className="card">
        <header className="card-h">
          <h2>
            <IconShield size={18} /> Computadores conhecidos
          </h2>
          <span className="chip">{settings?.known.length ?? 0}</span>
        </header>
        <div className="plist">
          {settings?.known.map((k) => (
            <div className="row-item" key={k.fingerprint}>
              <span className="ri-icon grad">
                <IconMonitor />
              </span>
              <span className="ri-text">
                <b>{k.name}</b>
                <small>
                  {k.side ? sideLabel(k.side) : 'posição não definida'}
                  {k.ip ? ` · ${k.ip}` : ''}
                </small>
              </span>
              <button
                className="btn mini icon"
                title="Esquecer"
                onClick={() => void api.forgetKnown(k.fingerprint)}
              >
                <IconTrash size={16} />
              </button>
            </div>
          ))}
          {(settings?.known.length ?? 0) === 0 && (
            <div className="empty">
              <span className="radar">
                <IconShield size={32} />
              </span>
              <small>Marque “não perguntar de novo” ao conectar para lembrar um computador.</small>
            </div>
          )}
        </div>
      </section>

      <section className="card">
        <header className="card-h">
          <h2>
            <IconInfo size={18} /> Sobre
          </h2>
        </header>
        <div className="plist">
          <div className="row-item">
            <span className="ri-icon">
              <IconRadar />
            </span>
            <span className="ri-text">
              <b>Descoberta</b>
              <small>
                {!settings?.visible
                  ? 'desligada'
                  : disc === null
                    ? 'iniciando…'
                    : !disc.listening
                      ? 'porta 47801 ocupada (use o IP)'
                      : `${disc.interfaces} rede(s) · ${disc.received} anúncio(s)`}
              </small>
            </span>
          </div>
          <div className="row-item">
            <span className="ri-icon warn">
              <IconKeyboard />
            </span>
            <span className="ri-text">
              <b>Atalho de emergência</b>
              <small>
                Ctrl + Alt + Shift + F12
                {info && !info.hotkey_registered ? ' (em uso por outro app)' : ''}
              </small>
            </span>
          </div>
          <div className="row-item">
            <span className="ri-icon">
              <IconFile />
            </span>
            <span className="ri-text">
              <b>Arquivos recebidos</b>
              <small>Pasta Downloads</small>
            </span>
          </div>
          <div className="row-item">
            <span className="ri-icon">
              <IconShield />
            </span>
            <span className="ri-text">
              <b>Privacidade</b>
              <small>Teclas digitadas nunca são gravadas</small>
            </span>
          </div>
        </div>
      </section>
    </div>
  );

  const tabs = [
    ['home', 'Painel', <IconDashboard size={16} key="h" />],
    ['connect', 'Conectar', <IconLink size={16} key="c" />],
    ['settings', 'Ajustes', <IconSliders size={16} key="s" />],
  ] as const;

  return (
    <div className="shell">
      <header className="topbar">
        <div className="brand">
          <Logo />
          <span>Windows Flow</span>
        </div>
        <nav className="navpill" role="tablist">
          {tabs.map(([id, label, icon]) => (
            <button
              key={id}
              role="tab"
              aria-selected={tab === id}
              className={tab === id ? 'on' : ''}
              onClick={() => setTab(id)}
            >
              {icon}
              {label}
            </button>
          ))}
        </nav>
        <div className="top-right">
          <span className={`pill pill-${tone}`}>
            <i />
            {connected ? (peerName ?? 'Conectado') : linkLabel(link)}
          </span>
          <span className="ver">v{info?.version}</span>
        </div>
      </header>

      <main className="content">
        <div className="title-row">
          <h1>{tab === 'home' ? 'Meu painel' : tab === 'connect' ? 'Conectar' : 'Ajustes'}</h1>
          <span className="muted">{info?.device_name}</span>
        </div>
        {(beingControlled || controlling) && (
          <div className="banner" role="alert">
            <IconInfo size={18} />
            <strong>
              {beingControlled
                ? 'Este PC está sendo controlado pelo outro computador.'
                : 'Você está controlando o outro computador.'}
            </strong>
            <button className="btn danger mini" onClick={() => void api.disconnect()}>
              Desconectar
            </button>
          </div>
        )}
        {tab === 'home' ? home : tab === 'connect' ? connectPage : settingsPage}
      </main>

      {confirm && (
        <div className="modal" role="dialog" aria-modal="true">
          <div className="dialog">
            <span className="dlg-ic">
              <IconShield size={26} />
            </span>
            <h2>{confirm.name ?? 'Um computador'} quer se conectar</h2>
            <p className="muted">
              Origem: <b>{confirm.peer}</b>. Confira se o código é <b>exatamente igual</b> ao do
              outro PC.
            </p>
            <div className="sas">{confirm.sas}</div>
            <label className="remember">
              <input
                type="checkbox"
                checked={remember}
                onChange={(e) => setRemember(e.target.checked)}
              />
              Confiar neste computador e <b>não perguntar de novo</b> (conecta sozinho e guarda a
              posição da tela)
            </label>
            <div className="dlg-actions">
              <button className="btn primary" onClick={() => void api.confirm(true, remember)}>
                <IconCheck size={16} /> Os códigos são iguais
              </button>
              <button className="btn danger" onClick={() => void api.confirm(false)}>
                Não são iguais
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
