//! Captura, supressão e injeção de entrada (Fase 1).
//!
//! Os tipos e o trait [`Platform`] são independentes do SO para que o motor em
//! `continuity-core` seja testável com uma plataforma falsa. A implementação
//! real para Windows está em [`windows_impl`].

pub use protocol::{ButtonState, PointerButton};

#[cfg(windows)]
pub mod windows_impl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Retângulo em pixels físicos; `right` e `bottom` são exclusivos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn center(&self) -> Point {
        Point {
            x: self.left + self.width() / 2,
            y: self.top + self.height() / 2,
        }
    }

    pub fn clamp(&self, p: Point) -> Point {
        Point {
            x: p.x.clamp(self.left, self.right - 1),
            y: p.y.clamp(self.top, self.bottom - 1),
        }
    }
}

/// Lado da tela em que o outro PC está posicionado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        }
    }

    pub fn parse(s: &str) -> Option<Side> {
        match s {
            "left" => Some(Side::Left),
            "right" => Some(Side::Right),
            "top" => Some(Side::Top),
            "bottom" => Some(Side::Bottom),
            _ => None,
        }
    }
}

/// Evento de entrada capturado localmente (enquanto REMOTE) ou a injetar no receptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Move {
        dx: i32,
        dy: i32,
    },
    Button {
        button: PointerButton,
        state: ButtonState,
    },
    Scroll {
        dx: i32,
        dy: i32,
    },
    Key {
        scan_code: u16,
        extended: bool,
        state: ButtonState,
    },
    /// Atalho de emergência pressionado. Sempre disponível, mesmo sem rede.
    Emergency,
    /// Com este PC sendo controlado: o usuário clicou, rolou ou digitou nele
    /// e quer retomar o controle local.
    Takeover,
}

/// Acesso do motor ao sistema de entrada e de janelas.
pub trait Platform: Send {
    /// Área da tela virtual (todos os monitores), em pixels físicos.
    fn screen(&self) -> Rect;
    fn cursor(&self) -> Point;
    /// Posiciona o cursor sem gerar movimento relativo (entrada/saída de borda).
    fn set_cursor(&mut self, p: Point);
    /// Move o cursor via injeção de entrada (movimentos vindos do par).
    fn move_cursor(&mut self, p: Point);
    /// Injeta botão, rolagem ou tecla. `Move` e `Emergency` são ignorados.
    fn inject(&mut self, ev: &InputEvent);
    /// Solta todas as teclas e botões injetados que ainda estão pressionados.
    fn release_all(&mut self);
    /// Liga/desliga captura e supressão das entradas locais. Ao desligar,
    /// restaura modificadores cujo "soltar" foi engolido.
    fn capture(&mut self, on: bool);
    /// No PC controlado: engole a entrada física (a injetada continua passando).
    /// Clique, rolagem ou tecla física gera [`InputEvent::Takeover`] e desliga o bloqueio.
    fn block_local(&mut self, _on: bool) {}
    /// Pede timers de alta resolução enquanto há sessão ativa.
    fn performance(&mut self, _on: bool) {}
    /// Prova de vida do motor. Sem isso por 2 s a captura se desliga sozinha.
    fn keepalive(&self);
}
