# Windows Flow

Compartilha mouse, teclado, clipboard e arquivos entre dois PCs Windows 10/11 na mesma LAN.

**Status: Fase 1 (prova de conceito de input em LAN), implementada e testada em um único PC.** A validação com dois PCs físicos ainda não foi feita: veja [docs/phase1-test-guide.md](docs/phase1-test-guide.md).

## Requisitos

Node 22+, npm 10+, Rust stable, MSVC Build Tools 2022, WebView2.

## Comandos

```
npm install
npm run dev                 # app Tauri em modo desenvolvimento
npm run build               # instaladores NSIS/MSI
npm run lint && npm run typecheck && npm test && npm run format:check
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Documentação: [arquitetura](docs/architecture.md) · [segurança](docs/security.md) · [plano de testes](docs/test-plan.md)
