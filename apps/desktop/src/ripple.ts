import type { PortalEvent, Side } from '@windows-flow/shared-types';

/** Duração da animação, em ms. */
export const DURATION_MS = 1300;
/** Raio máximo das ondas, em px. */
export const MAX_RADIUS = 560;

export const clamp01 = (x: number) => Math.min(1, Math.max(0, x));
export const easeOut = (x: number) => 1 - Math.pow(1 - clamp01(x), 3);
export const easeIn = (x: number) => Math.pow(clamp01(x), 3);

/** Progresso (0..1) da onda `k` no instante `t` (0..1); as ondas partem escalonadas. */
export const waveProgress = (t: number, k: number) => clamp01(t * 1.2 - k * 0.16);

/** Raio da onda: ao entrar ela se expande a partir do contato; ao sair, se contrai até ele. */
export function waveRadius(q: number, enter: boolean, maxRadius = MAX_RADIUS): number {
  return (enter ? easeOut(q) : 1 - easeIn(q)) * maxRadius;
}

/** As ondas cabem na janela: nunca ultrapassam ~95% da profundidade (o resto é degradê). */
export const maxRadiusFor = (depth: number) => Math.min(MAX_RADIUS, depth * 0.95);

export interface SideVectors {
  /** Ponto da borda onde a coordenada local v = 0 (em pixels da janela). */
  o: [number, number];
  /** Direção "para dentro da tela" (u crescente). */
  u: [number, number];
  /** Direção ao longo da borda (v crescente). */
  v: [number, number];
}

/** Referencial local de cada lado: u = distância da borda, v = posição ao longo dela. */
export function sideVectors(side: Side, w: number, h: number): SideVectors {
  switch (side) {
    case 'left':
      return { o: [0, 0], u: [1, 0], v: [0, 1] };
    case 'right':
      return { o: [w, 0], u: [-1, 0], v: [0, 1] };
    case 'top':
      return { o: [0, 0], u: [0, 1], v: [1, 0] };
    case 'bottom':
      return { o: [0, h], u: [0, -1], v: [1, 0] };
  }
}

const VERTEX = `
attribute vec2 aPos;
uniform vec2 uRes;
varying vec2 vPx;
void main() {
  vPx = vec2((aPos.x * 0.5 + 0.5) * uRes.x, (1.0 - (aPos.y * 0.5 + 0.5)) * uRes.y);
  gl_Position = vec4(aPos, 0.0, 1.0);
}`;

/**
 * Refração: o conteúdo capturado da tela é empurrado por ondas que partem do ponto de
 * contato, com um abaulamento inicial, aberração cromática e um brilho suave na borda.
 * Só os pixels deslocados ficam visíveis; o resto é transparente e mostra a tela real.
 */
const FRAGMENT = `
precision highp float;
varying vec2 vPx;
uniform sampler2D uTex;
uniform vec2 uRes;
uniform vec2 uO;
uniform vec2 uDirU;
uniform vec2 uDirV;
uniform float uV0;
uniform float uT;
uniform float uEnter;
uniform float uMotion;
uniform float uHasTex;
uniform float uDepth;
uniform float uSpan;
uniform float uMaxR;

float easeOut3(float x) { return 1.0 - pow(1.0 - x, 3.0); }
float easeIn3(float x) { return x * x * x; }
vec3 palette(float x) { return 0.5 + 0.5 * cos(6.28318 * (vec3(0.0, 0.33, 0.67) + x)); }

void main() {
  vec2 rel = vPx - uO;
  vec2 d = vec2(dot(rel, uDirU), dot(rel, uDirV) - uV0);
  float r = length(d);
  vec2 n = r > 0.5 ? d / r : vec2(1.0, 0.0);

  // Máscara: todo efeito se dissolve em degradê antes de chegar aos limites da janela
  // (fundo e extremidades), para nunca haver um corte visível. O envelope no tempo faz
  // o efeito nascer e morrer suavemente.
  float along = dot(rel, uDirV);
  float fade = smoothstep(0.0, 150.0, uDepth - d.x) * smoothstep(0.0, 170.0, min(along, uSpan - along));
  float master = smoothstep(0.0, 0.10, uT) * (1.0 - smoothstep(0.76, 1.0, uT));
  float amt = fade * fade * master * uMotion;
  // Abaulamento: o conteúdo junto ao contato é ampliado e relaxa.
  float bulgeEnv = sin(3.14159 * clamp(uT / 0.45, 0.0, 1.0));
  float bulge = exp(-(r * r) / (170.0 * 170.0)) * bulgeEnv * amt;
  vec2 dm = d * (1.0 - 0.16 * bulge);

  // Ondas de refração.
  vec2 disp = vec2(0.0);
  float crest = 0.0;
  for (int k = 0; k < 3; k++) {
    float q = clamp(uT * 1.2 - float(k) * 0.16, 0.0, 1.0);
    float prog = uEnter > 0.5 ? easeOut3(q) : 1.0 - easeIn3(q);
    float x = r - prog * uMaxR;
    float width = 64.0 + 36.0 * q;
    float g = exp(-(x * x) / (width * width));
    float env = sin(3.14159 * q);
    float wave = sin(x / width * 2.0);
    disp += n * (wave * g * env * 13.0 * amt);
    crest += g * env * (0.5 + 0.5 * wave);
  }

  vec2 sl = dm - disp;
  vec2 sp = uO + uDirU * sl.x + uDirV * (sl.y + uV0);
  vec2 off = sp - vPx;

  // Aberração cromática: cada canal é refratado com intensidade ligeiramente diferente.
  vec3 col = vec3(
    texture2D(uTex, clamp((vPx + off * 1.18) / uRes, 0.0, 1.0)).r,
    texture2D(uTex, clamp(sp / uRes, 0.0, 1.0)).g,
    texture2D(uTex, clamp((vPx + off * 0.82) / uRes, 0.0, 1.0)).b
  );
  float a = smoothstep(0.0, 3.0, length(off)) * uHasTex;
  col *= 1.0 + 0.18 * crest;

  // Brilho suave colado na borda e nas cristas das ondas.
  float glowEnv = sin(3.14159 * clamp(uT / 0.9, 0.0, 1.0));
  float edge = exp(-max(d.x, 0.0) / 38.0) * exp(-(d.y * d.y) / (300.0 * 300.0)) * glowEnv * fade * master;
  float amount = clamp(edge * 0.38 + crest * 0.09 * glowEnv * fade * master, 0.0, 0.6);
  vec3 tint = mix(vec3(1.0), palette(atan(d.y, d.x) / 6.28318 * 0.6 + uT * 0.4), 0.65);

  float outA = clamp(a + amount, 0.0, 1.0);
  vec3 outRgb = min(col * a + tint * amount, vec3(outA));
  gl_FragColor = vec4(outRgb, outA);
}`;

export interface Renderer {
  play(ev: PortalEvent, pixels: Uint8Array | null, reducedMotion: boolean): void;
  stop(): void;
}

function compile(gl: WebGLRenderingContext, type: number, src: string): WebGLShader {
  const sh = gl.createShader(type)!;
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
    throw new Error(gl.getShaderInfoLog(sh) ?? 'shader inválido');
  }
  return sh;
}

/** Cria o renderizador WebGL, ou `null` se o WebGL não estiver disponível. */
export function createRenderer(canvas: HTMLCanvasElement): Renderer | null {
  const gl = canvas.getContext('webgl', {
    alpha: true,
    premultipliedAlpha: true,
    antialias: false,
  });
  if (!gl) return null;

  const prog = gl.createProgram()!;
  gl.attachShader(prog, compile(gl, gl.VERTEX_SHADER, VERTEX));
  gl.attachShader(prog, compile(gl, gl.FRAGMENT_SHADER, FRAGMENT));
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    throw new Error(gl.getProgramInfoLog(prog) ?? 'programa inválido');
  }
  gl.useProgram(prog);

  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
  const aPos = gl.getAttribLocation(prog, 'aPos');
  gl.enableVertexAttribArray(aPos);
  gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);

  const u = (name: string) => gl.getUniformLocation(prog, name);
  const loc = {
    res: u('uRes'),
    o: u('uO'),
    dirU: u('uDirU'),
    dirV: u('uDirV'),
    v0: u('uV0'),
    t: u('uT'),
    enter: u('uEnter'),
    motion: u('uMotion'),
    hasTex: u('uHasTex'),
    depth: u('uDepth'),
    span: u('uSpan'),
    maxR: u('uMaxR'),
    tex: u('uTex'),
  };

  const tex = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, tex);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  gl.uniform1i(loc.tex, 0);

  let raf = 0;
  const clear = () => {
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
  };

  return {
    play(ev, pixels, reducedMotion) {
      cancelAnimationFrame(raf);
      canvas.width = ev.w;
      canvas.height = ev.h;
      gl.viewport(0, 0, ev.w, ev.h);
      gl.disable(gl.BLEND);

      const hasTex = pixels !== null && pixels.length === ev.w * ev.h * 4;
      gl.bindTexture(gl.TEXTURE_2D, tex);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      if (hasTex) {
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, ev.w, ev.h, 0, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
      } else {
        gl.texImage2D(
          gl.TEXTURE_2D,
          0,
          gl.RGBA,
          1,
          1,
          0,
          gl.RGBA,
          gl.UNSIGNED_BYTE,
          new Uint8Array(4),
        );
      }

      const vec = sideVectors(ev.side, ev.w, ev.h);
      gl.uniform2f(loc.res, ev.w, ev.h);
      gl.uniform2f(loc.o, vec.o[0], vec.o[1]);
      gl.uniform2f(loc.dirU, vec.u[0], vec.u[1]);
      gl.uniform2f(loc.dirV, vec.v[0], vec.v[1]);
      gl.uniform1f(loc.v0, ev.v0);
      gl.uniform1f(loc.enter, ev.enter ? 1 : 0);
      gl.uniform1f(loc.motion, reducedMotion ? 0 : 1);
      gl.uniform1f(loc.hasTex, hasTex ? 1 : 0);
      const vertical = ev.side === 'left' || ev.side === 'right';
      const depth = vertical ? ev.w : ev.h;
      gl.uniform1f(loc.depth, depth);
      gl.uniform1f(loc.span, vertical ? ev.h : ev.w);
      gl.uniform1f(loc.maxR, maxRadiusFor(depth));

      let start: number | null = null;
      const frame = (now: number) => {
        start ??= now;
        const t = clamp01((now - start) / DURATION_MS);
        gl.uniform1f(loc.t, t);
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
        if (t < 1) raf = requestAnimationFrame(frame);
        else clear();
      };
      raf = requestAnimationFrame(frame);
    },
    stop() {
      cancelAnimationFrame(raf);
      clear();
    },
  };
}

/**
 * Plano B sem WebGL: brilho na borda e ondas que se expandem/contraem a partir do contato.
 * Não distorce a tela (não há shader), mas a transição continua visível.
 */
export function createFallbackRenderer(canvas: HTMLCanvasElement): Renderer | null {
  const ctx = canvas.getContext('2d');
  if (!ctx) return null;
  let raf = 0;
  return {
    play(ev, _pixels, reducedMotion) {
      cancelAnimationFrame(raf);
      canvas.width = ev.w;
      canvas.height = ev.h;
      const vertical = ev.side === 'left' || ev.side === 'right';
      const depth = vertical ? ev.w : ev.h;
      const vec = sideVectors(ev.side, ev.w, ev.h);
      const maxR = maxRadiusFor(depth);
      let start: number | null = null;
      const frame = (now: number) => {
        start ??= now;
        const t = clamp01((now - start) / DURATION_MS);
        const master = clamp01(t / 0.1) * (1 - clamp01((t - 0.76) / 0.24));
        ctx.setTransform(1, 0, 0, 1, 0, 0);
        ctx.clearRect(0, 0, ev.w, ev.h);
        // referencial local: x = u (para dentro da tela), y = v (ao longo da borda)
        ctx.setTransform(vec.u[0], vec.u[1], vec.v[0], vec.v[1], vec.o[0], vec.o[1]);
        const glow = ctx.createRadialGradient(0, ev.v0, 0, 0, ev.v0, 260);
        glow.addColorStop(0, `rgba(160, 200, 255, ${0.5 * master})`);
        glow.addColorStop(1, 'rgba(160, 120, 255, 0)');
        ctx.fillStyle = glow;
        ctx.fillRect(0, ev.v0 - 260, 260, 520);
        if (!reducedMotion) {
          for (let k = 0; k < 3; k++) {
            const q = waveProgress(t, k);
            const r = waveRadius(q, ev.enter, maxR);
            if (q <= 0 || q >= 1 || r < 2) continue;
            ctx.beginPath();
            ctx.ellipse(0, ev.v0, r * 0.5, r, 0, -Math.PI / 2, Math.PI / 2);
            ctx.strokeStyle = `rgba(200, 225, 255, ${0.45 * Math.sin(Math.PI * q) * master})`;
            ctx.lineWidth = 3;
            ctx.stroke();
          }
        }
        if (t < 1) raf = requestAnimationFrame(frame);
        else {
          ctx.setTransform(1, 0, 0, 1, 0, 0);
          ctx.clearRect(0, 0, ev.w, ev.h);
        }
      };
      raf = requestAnimationFrame(frame);
    },
    stop() {
      cancelAnimationFrame(raf);
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, canvas.width, canvas.height);
    },
  };
}
