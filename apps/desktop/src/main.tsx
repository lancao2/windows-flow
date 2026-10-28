import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import { Portal } from './Portal';
import { Toast } from './Toast';
import { XferRecv, XferSend } from './Transfer';
import './styles.css';
import './dashboard.css';

// A mesma página serve a janela principal e o overlay do portal (#portal).
const isPortal = window.location.hash.startsWith('#portal');
const force2d = window.location.hash === '#portal-2d';
const isToast = window.location.hash === '#toast';
const xferKind =
  window.location.hash === '#xfer-send'
    ? 'send'
    : window.location.hash === '#xfer-recv'
      ? 'recv'
      : null;

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    {isToast ? (
      <Toast />
    ) : isPortal ? (
      <Portal force2d={force2d} />
    ) : xferKind === 'send' ? (
      <XferSend />
    ) : xferKind === 'recv' ? (
      <XferRecv />
    ) : (
      <App />
    )}
  </React.StrictMode>,
);
