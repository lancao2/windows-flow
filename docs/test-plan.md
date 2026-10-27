# Plano de testes

## Automatizados (Fase 0)

- `cargo test --workspace`: protocolo (limites, versão, malformado), máquina de estados (transições e eventos de segurança), sanitização de caminhos.
- `npm test`: testes do frontend (vitest).
- CI: format, lint, typecheck, clippy `-D warnings`, build web.

## Automatizados (Fase 1)

- `transport`: SAS idêntico nos dois lados, impressão digital fixada, rejeição de par errado, filtro LAN,
  frames malformados/gigantes, **par mudo detectado em < 2 s**, heartbeat repetido, primeiro frame != Hello.
- `continuity-core` (motor sobre TCP+TLS reais, plataforma falsa): pareamento, cruzar borda, movimento, teclas em ordem,
  retorno, atalho de emergência (inclusive sem par), queda do par < 2 s, desconexão do usuário em REMOTE,
  nunca injetar sem concessão, sequências repetidas ignoradas, `recover` idempotente.
- `windows-input` (`--ignored`, mexe no cursor): injeção absoluta cai no pixel pedido (±1 px); liga/desliga hooks sem eventos espúrios.

## Manuais (hardware real)

Veja [phase1-test-guide.md](phase1-test-guide.md). **Nenhum item foi executado em dois PCs ainda.**
Não verificável sem dois PCs/entrada física: captura e supressão reais pelos hooks, repasse de teclas, DPI misto, layouts de teclado.

## Matriz de suporte

| Item                        | Suportado                                  |
| --------------------------- | ------------------------------------------ |
| Windows 10/11 x64           | Sim (desenvolvido em Windows 11 Pro 26200) |
| Windows ARM, macOS, Linux   | Não                                        |
| Apps elevados / UAC / login | Não                                        |
