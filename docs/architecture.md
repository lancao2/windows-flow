# Arquitetura

Monorepo: `apps/desktop` (Tauri 2 + React), `crates/*` (Rust), `packages/shared-types` (TS).
`apps/api` (NestJS) só será criado na Fase 3.

| Crate           | Responsabilidade                                                                                               | Estado                                              |
| --------------- | -------------------------------------------------------------------------------------------------------------- | --------------------------------------------------- |
| protocol        | Mensagens versionadas (JSON), limites e validação                                                              | implementado + testes                               |
| continuity-core | Máquina de estados, geometria de bordas (histerese), motor assíncrono                                          | implementado + testes (25)                          |
| windows-input   | Trait `Platform` + implementação Windows: hooks LL, SendInput, RegisterHotKey, deadman                         | implementado; captura real só verificável com 2 PCs |
| transport       | TLS 1.3 mútuo efêmero + código SAS, framing, heartbeat 500 ms / timeout 1,5 s, reconexão com identidade fixada | implementado + testes (14)                          |
| discovery       | mDNS                                                                                                           | esqueleto (Fase 3)                                  |
| clipboard-sync  | Clipboard de texto                                                                                             | esqueleto (Fase 4)                                  |
| file-transfer   | Chunks, hash, sanitização de caminhos                                                                          | só sanitização (Fase 5)                             |

## Decisões

- **Transporte (a decidir na Fase 1):** PoC com TCP + TLS e framing explícito (menor complexidade);
  avaliar `quinn` (QUIC) depois, pois o mouse se beneficia de datagramas não confiáveis e as teclas de streams ordenados.
- **Codec:** JSON é provisório (legível, fácil de fuzzar). Mouse de alta frequência pode exigir codec binário; medir antes de trocar.
- **Invariante de segurança:** qualquer falha leva a RECOVERING; só REMOTE captura/suprime entrada (`State::captures_input`).
- **Plano de controle × dados:** autenticação/pareamento (backend) separados do tráfego P2P de eventos e arquivos.

## Suposições

- Windows 10/11 x64, sessão interativa, mesma LAN, sem elevação.

## Fase 1: como o controle passa de um PC para o outro

1. **Pareamento (sem login):** um PC escuta, o outro conecta por IP. TLS 1.3 mútuo com certificados efêmeros
   (nenhum é confiável por si). Os dois PCs mostram um código de 6 dígitos derivado do segredo exportado da sessão TLS;
   o usuário confirma que são iguais. Um intermediário teria chaves diferentes em cada ponta e os códigos divergiriam.
   Depois de confirmado, a impressão digital do par fica fixada **em memória** (reconexão automática sem novo código; some ao fechar o app).
2. **Borda:** o PC de origem consulta `GetCursorPos` (8 ms, só enquanto conectado). Cursor na borda configurada por 60 ms
   → `ControlRequest{entry}` (posição proporcional na borda). Fora da borda antes disso = histerese, nada acontece.
3. **Entrada no destino:** concede (`ControlGranted`) só se estiver LOCAL e livre; posiciona o cursor na própria borda
   (a que aponta para o par) na mesma proporção. Passa a ser "controlado".
4. **REMOTE na origem:** liga os hooks LL. O cursor físico fica parado no centro; cada movimento vira um delta
   (coalescido em ≥ 4 ms), cliques/teclas/rolagem seguem em ordem com `seq` crescente. Tudo é engolido localmente.
5. **Destino:** só injeta se concedeu o controle e `seq` é maior que o último. Movimento = `SendInput` absoluto (sem aceleração),
   teclas por scan code. Empurrar 15 px contra a borda de volta devolve o controle (`ControlReleased{exit}`).
6. **Recuperação:** `Engine::recover` desliga hooks, solta teclas/botões injetados, restaura modificadores e volta a LOCAL.
   Disparada por: atalho de emergência (`RegisterHotKey` + detecção dentro do hook), perda de heartbeat (1,5 s), erro de envio,
   fila cheia, falha parcial ao instalar hooks, desconexão do usuário, encerramento do app, ou **deadman** (sem sinal de vida do motor por 2 s).

## Limitações conhecidas da Fase 1

- Bordas: usa o retângulo da tela virtual inteira; layouts de monitores não retangulares podem ter "cantos" onde a borda externa não coincide com um monitor.
- Enquanto controlado, o PC bloqueia o mouse/teclado **físicos** (só passa o que foi injetado). Clicar, rolar ou digitar nele retoma o controle (`Takeover`); mover o mouse sozinho não. Ctrl+Alt+Del continua funcionando.
- Teclas por scan code: o layout de teclado do **destino** define o caractere (ABNT2 → US gera caracteres diferentes).
- Bloqueio de tela/troca de usuário/suspensão: sem tratamento dedicado ainda; o heartbeat e o deadman cobrem quando o processo para, mas não há evento de sessão (previsto para Fase 6).
- Apps elevados, UAC e tela de login não recebem entrada (UIPI). Jogos em tela cheia exclusiva podem ignorar hooks e `SendInput`.
- Confiança fixada só em memória; perfil persistente e revogação ficam para a Fase 3.

## Correções e acréscimos pós-teste (Fase 1)

- **Retorno e nova travessia:** ao devolver o controle, o PC que estava sendo controlado ficava com o cursor parado na borda de saída e o detector de borda dele interpretava isso como intenção de cruzar de volta (ARMED → pedido de controle ao outro PC). Agora `end_control` desarma a borda e exige afastar-se 24 px antes de rearmar. Coberto por teste de regressão (3 idas e voltas seguidas).
- **Janela minimizada:** o Windows 11 reduz prioridade e resolução de timer de processos minimizados, o que atrasa heartbeat (timeout de 1,5 s) e movimento. O processo desliga o _power throttling_ ao iniciar e pede timers de 1 ms enquanto há sessão ativa. (Hipótese de causa; precisa de confirmação em campo.)
- **Efeito de transição (distorção da tela, estilo iOS):** ao cruzar a borda, o app captura em memória a faixa da tela junto ao ponto de contato (GDI `BitBlt`, no máximo 1000 px ao longo da borda), abre uma janela transparente, sem foco e que ignora o mouse, e um shader WebGL refrata esses pixels: abaulamento no contato, três ondas (expandem ao entrar, contraem ao sair), aberração cromática e brilho suave na borda. Só os pixels deslocados ficam visíveis, então o resto é a tela real (sem congelar vídeo). A captura nunca é enviada pela rede nem gravada e é descartada ao fim da animação. Pode ser desligada na UI; respeita `prefers-reduced-motion` (só o brilho). Flag de QA: `windows-flow.exe --preview-portal`.
- **Sem cortes visíveis (0.2.1):** todo o efeito é multiplicado por uma máscara com degradê (150 px no fundo da janela, 170 px nas pontas) e por um envelope no tempo (nasce e morre suavemente); as ondas nunca passam de 95% da profundidade da janela e a opacidade da distorção cresce gradualmente com o deslocamento. O posicionamento do overlay é conferido (tamanho/posição reais) e repetido se o Windows redimensionar a janela por diferença de escala (DPI) entre monitores.
- **Robustez da animação (0.2.2):** a 0.2.1 instalada pelo usuário era uma build intermediária com erro de compilação no shader GLSL, o que fez a animação sumir em silêncio (e a build corrigida saiu com o mesmo número de versão: cada build agora tem versão própria). Agora: (1) o cronômetro da animação parte do primeiro quadro desenhado, para que uma página congelada/oculta que acorde tarde não pule direto para o fim; (2) se o WebGL estiver indisponível ou o shader falhar, usa um plano B em Canvas 2D (brilho + ondas, sem distorção do conteúdo); (3) o overlay reporta seu estado (`ok: WebGL`, plano B, erro) e cada animação tocada ao app, que mostra "Estado da animação" na tela e grava um log pequeno em `%TEMP%\windows-flow-portal.log` (só geometria, tempos e erros; reinicia ao passar de 64 KB). Flags de QA: `--preview-portal[=left|right|top|bottom]`, `--preview-delay=SEGUNDOS`, `--force-2d`.
