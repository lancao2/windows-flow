//! Geometria de bordas, detecção com histerese e detecção de retorno.

use protocol::MAX_RATIO;
use windows_input::{Point, Rect, Side};

/// Distância (em pixels) do ponto até a borda `side`; 0 significa "na borda".
pub fn edge_distance(p: Point, s: Rect, side: Side) -> i32 {
    match side {
        Side::Left => p.x - s.left,
        Side::Right => s.right - 1 - p.x,
        Side::Top => p.y - s.top,
        Side::Bottom => s.bottom - 1 - p.y,
    }
}

/// Posição ao longo da borda, de 0 a [`MAX_RATIO`].
pub fn ratio_along(p: Point, s: Rect, side: Side) -> u16 {
    let (v, lo, len) = match side {
        Side::Left | Side::Right => (p.y, s.top, s.height()),
        Side::Top | Side::Bottom => (p.x, s.left, s.width()),
    };
    let span = i64::from((len - 1).max(1));
    let r = (i64::from(v - lo) * i64::from(MAX_RATIO) / span).clamp(0, i64::from(MAX_RATIO));
    r as u16
}

/// Ponto na borda `side`, na posição proporcional `ratio`, `inset` pixels para dentro.
pub fn point_on_edge(s: Rect, side: Side, ratio: u16, inset: i32) -> Point {
    let along = |lo: i32, len: i32| {
        lo + (i64::from(ratio) * i64::from((len - 1).max(0)) / i64::from(MAX_RATIO)) as i32
    };
    let p = match side {
        Side::Left => Point {
            x: s.left + inset,
            y: along(s.top, s.height()),
        },
        Side::Right => Point {
            x: s.right - 1 - inset,
            y: along(s.top, s.height()),
        },
        Side::Top => Point {
            x: along(s.left, s.width()),
            y: s.top + inset,
        },
        Side::Bottom => Point {
            x: along(s.left, s.width()),
            y: s.bottom - 1 - inset,
        },
    };
    s.clamp(p)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeSignal {
    None,
    /// Cursor encostou na borda (estado ARMED).
    Entered,
    /// Cursor se afastou antes de completar o tempo de permanência (histerese).
    Left,
    /// Permaneceu na borda pelo tempo configurado: pedir o controle.
    Cross(u16),
}

/// Detecta a intenção de cruzar a borda: o cursor precisa ficar na borda por
/// `dwell_ms`. Após um retorno, só rearma depois de se afastar `rearm_px`.
#[derive(Debug)]
pub struct EdgeDetector {
    side: Side,
    dwell_ms: u64,
    rearm_px: i32,
    since: Option<u64>,
    fired: bool,
    blocked: bool,
}

impl EdgeDetector {
    pub fn new(side: Side, dwell_ms: u64, rearm_px: i32) -> Self {
        Self {
            side,
            dwell_ms,
            rearm_px,
            since: None,
            fired: false,
            blocked: false,
        }
    }

    pub fn set_side(&mut self, side: Side) {
        self.side = side;
        self.block();
    }

    /// Exige afastamento da borda antes de disparar de novo.
    pub fn block(&mut self) {
        self.blocked = true;
        self.since = None;
        self.fired = false;
    }

    pub fn update(&mut self, now_ms: u64, p: Point, screen: Rect) -> EdgeSignal {
        let d = edge_distance(p, screen, self.side);
        if self.blocked {
            if d >= self.rearm_px {
                self.blocked = false;
            } else {
                return EdgeSignal::None;
            }
        }
        if d <= 0 {
            match self.since {
                None => {
                    self.since = Some(now_ms);
                    self.fired = false;
                    EdgeSignal::Entered
                }
                Some(t) if !self.fired && now_ms.saturating_sub(t) >= self.dwell_ms => {
                    self.fired = true;
                    EdgeSignal::Cross(ratio_along(p, screen, self.side))
                }
                Some(_) => EdgeSignal::None,
            }
        } else if self.since.take().is_some() {
            let was_fired = std::mem::take(&mut self.fired);
            if was_fired {
                EdgeSignal::None
            } else {
                EdgeSignal::Left
            }
        } else {
            EdgeSignal::None
        }
    }
}

/// No PC controlado: detecta o usuário empurrando o cursor para fora pela borda
/// de volta ao PC de origem.
#[derive(Debug)]
pub struct ReturnDetector {
    side: Side,
    push_px: i32,
    accum: i32,
}

impl ReturnDetector {
    pub fn new(side: Side, push_px: i32) -> Self {
        Self {
            side,
            push_px,
            accum: 0,
        }
    }

    pub fn set_side(&mut self, side: Side) {
        self.side = side;
        self.accum = 0;
    }

    pub fn reset(&mut self) {
        self.accum = 0;
    }

    /// `pos` é a posição após aplicar o movimento; `(dx, dy)`, o delta pedido.
    pub fn update(&mut self, pos: Point, screen: Rect, dx: i32, dy: i32) -> bool {
        let outward = match self.side {
            Side::Left => -dx,
            Side::Right => dx,
            Side::Top => -dy,
            Side::Bottom => dy,
        };
        // 1 px de tolerância: a injeção absoluta pode errar um pixel.
        let at_edge = edge_distance(pos, screen, self.side) <= 1;
        if at_edge && outward > 0 {
            self.accum += outward;
        } else if !at_edge || outward < 0 {
            self.accum = 0;
        }
        self.accum >= self.push_px
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    const P: fn(i32, i32) -> Point = |x, y| Point { x, y };

    #[test]
    fn ratio_roundtrip_on_vertical_edge() {
        for y in [0, 1, 540, 1079] {
            let r = ratio_along(P(1919, y), S, Side::Right);
            let back = point_on_edge(S, Side::Right, r, 0);
            assert!((back.y - y).abs() <= 1, "{y} -> {r} -> {}", back.y);
            assert_eq!(back.x, 1919);
        }
    }

    #[test]
    fn entry_is_inside_the_screen() {
        for side in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
            let p = point_on_edge(S, side, 10_000, 3);
            assert!(
                p.x >= 0 && p.x < 1920 && p.y >= 0 && p.y < 1080,
                "{side:?} {p:?}"
            );
        }
        assert_eq!(point_on_edge(S, Side::Left, 0, 2), P(2, 0));
    }

    #[test]
    fn works_with_negative_origin_monitors() {
        let s = Rect {
            left: -1920,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert_eq!(edge_distance(P(-1920, 5), s, Side::Left), 0);
        assert_eq!(point_on_edge(s, Side::Left, 0, 0).x, -1920);
    }

    #[test]
    fn dwell_then_cross() {
        let mut d = EdgeDetector::new(Side::Right, 60, 24);
        assert_eq!(d.update(0, P(1919, 540), S), EdgeSignal::Entered);
        assert_eq!(d.update(30, P(1919, 540), S), EdgeSignal::None);
        assert_eq!(d.update(60, P(1919, 540), S), EdgeSignal::Cross(5004));
        // Não dispara de novo enquanto permanece.
        assert_eq!(d.update(200, P(1919, 540), S), EdgeSignal::None);
    }

    #[test]
    fn brushing_the_edge_does_not_cross() {
        let mut d = EdgeDetector::new(Side::Right, 60, 24);
        assert_eq!(d.update(0, P(1919, 10), S), EdgeSignal::Entered);
        assert_eq!(d.update(20, P(1900, 10), S), EdgeSignal::Left);
        assert_eq!(d.update(40, P(1919, 10), S), EdgeSignal::Entered);
    }

    #[test]
    fn rearm_requires_distance_after_block() {
        let mut d = EdgeDetector::new(Side::Right, 0, 24);
        d.block();
        assert_eq!(d.update(0, P(1919, 5), S), EdgeSignal::None);
        assert_eq!(d.update(10, P(1910, 5), S), EdgeSignal::None);
        assert_eq!(d.update(20, P(1890, 5), S), EdgeSignal::None); // rearmou, longe da borda
        assert_eq!(d.update(30, P(1919, 5), S), EdgeSignal::Entered);
    }

    #[test]
    fn other_edges_are_ignored() {
        let mut d = EdgeDetector::new(Side::Right, 0, 24);
        assert_eq!(d.update(0, P(0, 500), S), EdgeSignal::None);
        assert_eq!(d.update(0, P(500, 0), S), EdgeSignal::None);
    }

    #[test]
    fn return_needs_a_deliberate_push() {
        let mut r = ReturnDetector::new(Side::Left, 15);
        // Longe da borda: nunca retorna.
        assert!(!r.update(P(500, 500), S, -100, 0));
        // Encostou, mas só um toque.
        assert!(!r.update(P(0, 500), S, -5, 0));
        assert!(!r.update(P(0, 500), S, -5, 0));
        assert!(r.update(P(0, 500), S, -6, 0));
    }

    #[test]
    fn return_resets_when_moving_away() {
        let mut r = ReturnDetector::new(Side::Left, 15);
        assert!(!r.update(P(0, 500), S, -10, 0));
        assert!(!r.update(P(20, 500), S, 20, 0));
        assert!(!r.update(P(0, 500), S, -10, 0));
    }
}
