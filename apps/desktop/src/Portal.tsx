import { useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { PortalEvent } from '@windows-flow/shared-types';
import { createFallbackRenderer, createRenderer, type Renderer } from './ripple';

/** Registra o estado da animação no app principal e no log de diagnóstico (sem dados sensíveis). */
const report = (msg: string) => void invoke('portal_report', { msg }).catch(() => undefined);

/**
 * Conteúdo da janela de overlay (transparente): deforma a faixa de tela capturada com
 * ondas de refração a partir do ponto em que o cursor cruzou a borda. Se o WebGL não
 * estiver disponível ou o shader falhar, usa um plano B em Canvas 2D.
 */
export function Portal({ force2d = false }: { force2d?: boolean }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    document.body.classList.add('portal-body');
    const canvas = canvasRef.current;
    if (!canvas) return;

    let renderer: Renderer | null = null;
    try {
      renderer = createRenderer(canvas);
      report(renderer ? 'ok: WebGL' : 'WebGL indisponível; usando plano B (2D)');
    } catch (e) {
      report(`shader falhou: ${String(e)}; usando plano B (2D)`);
    }
    if (!renderer) {
      // Um canvas que já recebeu contexto 'webgl' não aceita '2d': recria o elemento.
      const fresh = document.createElement('canvas');
      fresh.className = canvas.className;
      canvas.replaceWith(fresh);
      renderer = createFallbackRenderer(fresh);
    }
    const rend = renderer;
    if (!rend) {
      report('nenhum renderizador disponível');
      return;
    }

    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    void listen<PortalEvent>('portal', async (e) => {
      const t0 = performance.now();
      let pixels: Uint8Array | null = null;
      try {
        pixels = new Uint8Array(await invoke<ArrayBuffer>('portal_snapshot'));
      } catch {
        pixels = null; // sem captura: só o brilho
      }
      if (cancelled) return;
      rend.play(e.payload, pixels, reduced);
      report(
        `play ${e.payload.side} enter=${e.payload.enter} ${e.payload.w}x${e.payload.h} ` +
          `pixels=${pixels?.length ?? 0} visivel=${document.visibilityState} ` +
          `preparo=${Math.round(performance.now() - t0)}ms`,
      );
    }).then((u) => (cancelled ? u() : (unlisten = u)));

    return () => {
      cancelled = true;
      unlisten?.();
      rend.stop();
    };
  }, []);

  return <canvas ref={canvasRef} className="portal-canvas" />;
}
