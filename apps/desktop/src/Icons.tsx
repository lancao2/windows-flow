import type { ReactNode, SVGProps } from 'react';

/** Conjunto de ícones SVG (traço de 1,8 px, 24x24) usado em todo o painel. */
type P = SVGProps<SVGSVGElement> & { size?: number };

function Svg({ size = 20, children, ...rest }: P & { children: ReactNode }) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      {...rest}
    >
      {children}
    </svg>
  );
}

export const IconDashboard = (p: P) => (
  <Svg {...p}>
    <rect x="3" y="3" width="7.5" height="9" rx="2" />
    <rect x="13.5" y="3" width="7.5" height="5.5" rx="2" />
    <rect x="13.5" y="11.5" width="7.5" height="9.5" rx="2" />
    <rect x="3" y="15" width="7.5" height="6" rx="2" />
  </Svg>
);

export const IconLink = (p: P) => (
  <Svg {...p}>
    <path d="M10 13a5 5 0 0 0 7.07 0l2.12-2.12a5 5 0 0 0-7.07-7.07L11 5" />
    <path d="M14 11a5 5 0 0 0-7.07 0L4.8 13.12a5 5 0 0 0 7.07 7.07L13 19" />
  </Svg>
);

export const IconSliders = (p: P) => (
  <Svg {...p}>
    <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
    <circle cx="15" cy="7" r="2" />
    <circle cx="9" cy="17" r="2" />
  </Svg>
);

export const IconMonitor = (p: P) => (
  <Svg {...p}>
    <rect x="3" y="4" width="18" height="12" rx="2.2" />
    <path d="M8 20h8M12 16v4" />
  </Svg>
);

export const IconWifi = (p: P) => (
  <Svg {...p}>
    <path d="M2.5 9.2a15 15 0 0 1 19 0M5.6 12.6a10.3 10.3 0 0 1 12.8 0M8.8 16a5.4 5.4 0 0 1 6.4 0" />
    <circle cx="12" cy="19.4" r="1" fill="currentColor" stroke="none" />
  </Svg>
);

export const IconCopy = (p: P) => (
  <Svg {...p}>
    <rect x="9" y="9" width="11.5" height="11.5" rx="2.2" />
    <path d="M5 15V6.5A2.5 2.5 0 0 1 7.5 4H16" />
  </Svg>
);

export const IconCheck = (p: P) => (
  <Svg {...p}>
    <path d="m5 12.5 4.5 4.5L19 7.5" />
  </Svg>
);

export const IconSend = (p: P) => (
  <Svg {...p}>
    <path d="M21.5 2.5 10.5 13.5" />
    <path d="m21.5 2.5-6.8 19-4.2-8-8-4.2z" />
  </Svg>
);

export const IconFile = (p: P) => (
  <Svg {...p}>
    <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
    <path d="M14 3v5h5" />
  </Svg>
);

export const IconFolder = (p: P) => (
  <Svg {...p}>
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V18a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
  </Svg>
);

export const IconShield = (p: P) => (
  <Svg {...p}>
    <path d="M12 3 4.5 6v5.5c0 4.4 3.1 8.3 7.5 9.5 4.4-1.2 7.5-5.1 7.5-9.5V6z" />
    <path d="m9 12 2.2 2.2L15.5 10" />
  </Svg>
);

export const IconKeyboard = (p: P) => (
  <Svg {...p}>
    <rect x="2" y="6" width="20" height="12" rx="2.4" />
    <path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7.5 14h9" />
  </Svg>
);

export const IconArrowRight = (p: P) => (
  <Svg {...p}>
    <path d="M5 12h14M13 6l6 6-6 6" />
  </Svg>
);

export const IconPower = (p: P) => (
  <Svg {...p}>
    <path d="M12 3v8" />
    <path d="M6.4 6.4a8 8 0 1 0 11.2 0" />
  </Svg>
);

export const IconTrash = (p: P) => (
  <Svg {...p}>
    <path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3" />
  </Svg>
);

export const IconBolt = (p: P) => (
  <Svg {...p}>
    <path d="M13 2.5 4.5 14H11l-1 7.5L19.5 10H13z" />
  </Svg>
);

export const IconClock = (p: P) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 7v5l3.2 2" />
  </Svg>
);

export const IconRadar = (p: P) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="9" />
    <circle cx="12" cy="12" r="5" />
    <path d="M12 12 18.5 5.5" />
    <circle cx="12" cy="12" r="1" fill="currentColor" stroke="none" />
  </Svg>
);

export const IconSparkles = (p: P) => (
  <Svg {...p}>
    <path d="m12 3 1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9z" />
    <path d="M19 16v4M17 18h4" />
  </Svg>
);

export const IconSwap = (p: P) => (
  <Svg {...p}>
    <path d="M7 4 3 8l4 4M3 8h14M17 20l4-4-4-4M21 16H7" />
  </Svg>
);

export const IconDownload = (p: P) => (
  <Svg {...p}>
    <path d="M12 4v11M7 11l5 5 5-5M4 20h16" />
  </Svg>
);

export const IconPlus = (p: P) => (
  <Svg {...p}>
    <path d="M12 5v14M5 12h14" />
  </Svg>
);

export const IconLock = (p: P) => (
  <Svg {...p}>
    <rect x="4.5" y="10.5" width="15" height="10" rx="2.4" />
    <path d="M8 10.5V8a4 4 0 0 1 8 0v2.5" />
  </Svg>
);

export const IconHand = (p: P) => (
  <Svg {...p}>
    <path d="M8 12V6.5a1.5 1.5 0 0 1 3 0V11M11 10V5a1.5 1.5 0 0 1 3 0v6M14 10.5V7a1.5 1.5 0 0 1 3 0v8a6 6 0 0 1-6 6h-1a6 6 0 0 1-5-2.7L3.5 14.5a1.6 1.6 0 0 1 2.6-1.8L8 15" />
  </Svg>
);

export const IconInfo = (p: P) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 11v5M12 8h.01" />
  </Svg>
);

/** Logotipo: dois monitores ligados por um fluxo, sobre um quadrado em degradê. */
export function Logo({ size = 34 }: { size?: number }) {
  return (
    <svg viewBox="0 0 40 40" width={size} height={size} aria-hidden>
      <defs>
        <linearGradient id="wf-logo" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#6ee7b7" />
          <stop offset="1" stopColor="#3b82f6" />
        </linearGradient>
      </defs>
      <rect width="40" height="40" rx="11" fill="url(#wf-logo)" />
      <rect x="7" y="10" width="14" height="10" rx="2.4" fill="#0e1520" opacity=".92" />
      <rect x="19" y="20" width="14" height="10" rx="2.4" fill="#0e1520" />
      <path
        d="M22 15h5.5a1.5 1.5 0 0 1 1.5 1.5V18"
        fill="none"
        stroke="#fff"
        strokeWidth="1.8"
        strokeLinecap="round"
      />
    </svg>
  );
}
