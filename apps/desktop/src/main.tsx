import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import { Portal } from './Portal';
import './styles.css';

// A mesma página serve a janela principal e o overlay do portal (#portal).
const isPortal = window.location.hash.startsWith('#portal');
const force2d = window.location.hash === '#portal-2d';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>{isPortal ? <Portal force2d={force2d} /> : <App />}</React.StrictMode>,
);
