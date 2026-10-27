//! Máquina de estados LOCAL -> ARMED -> REMOTE -> RECOVERING -> LOCAL.
//!
//! Puramente lógica (sem I/O) para ser testável. Invariante central: qualquer
//! falha, atalho de emergência ou desligamento leva a `Recovering`, e daí a
//! `Local`, onde nenhuma entrada é capturada nem suprimida.

pub mod engine;
pub mod geometry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Local,
    Armed,
    Remote,
    Recovering,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Local => "local",
            State::Armed => "armed",
            State::Remote => "remote",
            State::Recovering => "recovering",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Cursor entrou na zona da borda configurada.
    EdgeEntered,
    /// Cursor saiu da zona da borda antes de cruzar (histerese).
    EdgeLeft,
    /// Borda mantida pelo tempo de debounce e peer autorizou o controle.
    ControlGranted,
    /// Peer devolveu o controle (cursor cruzou de volta).
    ControlReleased,
    /// Perda de heartbeat/conexão, erro de injeção, bloqueio de sessão etc.
    Failure,
    /// Atalho de emergência local.
    EmergencyHotkey,
    /// Encerramento, suspensão ou logoff.
    Shutdown,
    /// Hooks liberados e estado de teclas/botões limpo.
    RecoveryComplete,
}

impl State {
    /// Transição pura. Eventos inválidos para o estado atual são ignorados,
    /// exceto os de segurança, que sempre levam a `Recovering`.
    pub fn next(self, event: Event) -> State {
        use Event::*;
        use State::*;
        match (self, event) {
            (Local | Armed | Remote, Failure | EmergencyHotkey | Shutdown) => Recovering,
            (Recovering, RecoveryComplete) => Local,
            (Recovering, _) => Recovering,
            (Local, EdgeEntered) => Armed,
            (Armed, EdgeLeft) => Local,
            (Armed, ControlGranted) => Remote,
            (Remote, ControlReleased) => Recovering,
            (s, _) => s,
        }
    }

    /// Entradas locais só podem ser capturadas/suprimidas em `Remote`.
    pub fn captures_input(self) -> bool {
        self == State::Remote
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path() {
        let mut s = State::Local;
        for (ev, want) in [
            (Event::EdgeEntered, State::Armed),
            (Event::ControlGranted, State::Remote),
            (Event::ControlReleased, State::Recovering),
            (Event::RecoveryComplete, State::Local),
        ] {
            s = s.next(ev);
            assert_eq!(s, want);
        }
    }

    #[test]
    fn hysteresis_returns_to_local() {
        assert_eq!(State::Armed.next(Event::EdgeLeft), State::Local);
    }

    #[test]
    fn safety_events_always_recover() {
        for s in [State::Local, State::Armed, State::Remote] {
            for ev in [Event::Failure, Event::EmergencyHotkey, Event::Shutdown] {
                assert_eq!(s.next(ev), State::Recovering);
            }
        }
    }

    #[test]
    fn recovering_ignores_everything_but_completion() {
        assert_eq!(
            State::Recovering.next(Event::ControlGranted),
            State::Recovering
        );
        assert_eq!(
            State::Recovering.next(Event::EdgeEntered),
            State::Recovering
        );
    }

    #[test]
    fn only_remote_captures_input() {
        assert!(State::Remote.captures_input());
        for s in [State::Local, State::Armed, State::Recovering] {
            assert!(!s.captures_input());
        }
    }

    #[test]
    fn cannot_enter_remote_without_arming() {
        assert_eq!(State::Local.next(Event::ControlGranted), State::Local);
    }
}
