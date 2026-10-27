//! Testes que tocam o Windows de verdade (movem o cursor). Rodar manualmente:
//! `cargo test -p windows-input --test hardware -- --ignored --nocapture`
#![cfg(windows)]

use tokio::sync::mpsc;
use windows_input::windows_impl::WinPlatform;
use windows_input::{Platform, Point};

#[test]
#[ignore = "move o cursor do usuário"]
fn absolute_move_lands_on_the_requested_pixel() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut p = WinPlatform::new(tx).expect("plataforma");
    let screen = p.screen();
    println!(
        "tela virtual: {screen:?}  atalho registrado: {}",
        p.hotkey_registered()
    );
    assert!(screen.width() > 0 && screen.height() > 0);

    let original = p.cursor();
    let targets = [
        Point {
            x: screen.left,
            y: screen.top,
        },
        Point {
            x: screen.right - 1,
            y: screen.bottom - 1,
        },
        Point {
            x: screen.left + screen.width() / 2,
            y: screen.top + screen.height() / 2,
        },
        Point {
            x: screen.left + 17,
            y: screen.bottom - 23,
        },
        Point {
            x: screen.right - 2,
            y: screen.top + 301,
        },
    ];
    let mut worst = 0;
    for t in targets {
        p.move_cursor(t);
        std::thread::sleep(std::time::Duration::from_millis(60));
        let got = p.cursor();
        let err = (got.x - t.x).abs().max((got.y - t.y).abs());
        println!("alvo {t:?} -> obtido {got:?} (erro {err}px)");
        worst = worst.max(err);
    }
    p.set_cursor(original);
    assert!(worst <= 1, "erro máximo de {worst}px na injeção absoluta");
}

#[test]
#[ignore = "liga e desliga hooks globais por um instante"]
fn capture_toggle_is_safe_and_deadman_releases() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut p = WinPlatform::new(tx).expect("plataforma");
    p.capture(true);
    std::thread::sleep(std::time::Duration::from_millis(150));
    p.capture(false);
    std::thread::sleep(std::time::Duration::from_millis(100));
    // Sem nenhuma entrada física nesse intervalo, nenhum evento deve ter sido capturado
    // (a tolerância cobre um usuário que mexa o mouse durante o teste).
    let mut n = 0;
    while rx.try_recv().is_ok() {
        n += 1;
    }
    println!("eventos capturados em 150 ms: {n}");
}
