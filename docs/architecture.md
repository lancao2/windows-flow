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

## Transferência de arquivos (0.3.0)

- **Enviar:** segure o botão esquerdo e arraste arquivos até a borda do outro PC. Encostado na borda (≤ 40 px), o anel completa em 2 s e envia; sair da borda, soltar ou sair da janela antes disso cancela. Com o botão pressionado o cursor **não cruza** a borda (evita pegar o controle no meio do arrasto). A janela `xfer-send` só existe enquanto o botão está pressionado perto da borda e recebe o arrasto via `DragDrop` do Tauri (lista de caminhos).
- **Receber:** a janela `xfer-recv` mostra, 0,9 s depois da animação de saída, o balão com anel de andamento (animação de ondas estilo AirDrop), que vira uma bandeja compacta. Arrastar o arquivo da bandeja usa `DoDragDrop` nativo (OLE, `windows-input/src/shell.rs`); ao ser aceito, a linha some e, sem mais arquivos, a bandeja fecha. O arquivo também fica em Downloads (`<nome>.wfpart` até completar; nomes repetidos viram `nome (1).ext`).
- **Protocolo (v2):** `FileOffer`/`FileChunk` (base64, 32 KiB)/`FileAck`/`FileCancel` no mesmo canal TLS, com janela de 512 KiB sem confirmação para não atrasar heartbeat e mouse. Só arquivos comuns (pastas são ignoradas); nomes validados contra caminhos e nomes reservados do Windows. PCs 0.2.x e 0.3.0 não se conectam (versão de protocolo diferente).
- Flag de QA: `--preview-xfer` mostra o envio (borda direita) e a chegada (borda esquerda) num só PC.

### 0.3.2

- O WebView2 engole o arrasto de arquivos antes de o Tauri vê-lo, então a captura passou para uma janela Win32 nativa (`DropCatcher`, alvo `IDropTarget`, quase invisível) rente à borda; as janelas web ficam só para o visual e deixam o mouse passar. Só aceita "copiar": o arquivo de origem nunca é movido.
- Sem anéis azuis: ao completar o anel, o balão some e toca a mesma animação de portal da troca de tela (saída aqui, chegada 0,9 s depois no outro PC).
- Flag de QA `--xfer-qa` finge um par conectado para testar o arrastar numa só máquina; os passos vão para `%TEMP%\windows-flow-portal.log`.

### 0.3.4

- Com este PC sendo controlado pelo mouse do outro, também dá para arrastar arquivos daqui: a janela de captura não depende mais de o PC estar "livre", e encostar na borda com o botão pressionado (ou durante uma transferência) não devolve o controle antes de o anel de 2 s completar.

### 0.3.5

- Tela principal redesenhada: conexão em abas ("Este PC aguarda" / "Conectar a outro PC") com o endereço em destaque e botão copiar; **mapa de posição** onde se arrasta o "Outro PC" para o lado desejado (eventos de ponteiro; clicar num encaixe também funciona; travado enquanto um PC controla o outro); guia rápido de uso; animação, eventos e segurança em "Opções avançadas".

### 0.3.6 — conectar sem digitar IP

- **Último PC:** ao concluir uma conexão, o IP do par é salvo em `%APPDATA%\br.ind.natuhair.windowsflow\settings.json` (`last_peer`, `visible`, `auto_connect`). Ao abrir, o app reconecta sozinho a ele (só o PC de menor IP toma a iniciativa; o outro aguarda) e repete a tentativa a cada 12 s enquanto estiver ocioso. "Desconectar" manual pausa a reconexão automática até o usuário conectar de novo.
- **Visível na rede:** por padrão o app já abre aguardando conexões (porta 47800) e anuncia nome/porta por UDP broadcast (porta 47801, `crates/discovery`). Anúncios só aceitos de IPs da LAN, da mesma versão de protocolo, e **nunca provam identidade**: toda conexão nova ainda exige conferir o código (SAS) nos dois PCs. Desligável em "Opções avançadas".
- **Aviso:** ao aparecer um PC com o app, um toast no canto inferior direito oferece "Conectar" (um clique, sem IP) ou "Agora não"; a lista "Computadores encontrados" na tela principal faz o mesmo. Não avisa sobre o último PC se a reconexão automática está ligada, nem quando a janela principal está em foco.
- Limitação: a identidade criptográfica é gerada a cada abertura, então o código de verificação precisa ser confirmado a cada nova sessão.

### 0.3.7 — computadores conhecidos

- **Identidade fixa:** o certificado/chave de cada PC passa a ser gravado em `%APPDATA%\br.ind.natuhair.windowsflow\identity.bin` (antes era novo a cada abertura). A impressão digital dele é o que identifica o computador.
- **"Não perguntar de novo":** no aviso de PC encontrado e no diálogo do código há a opção de confiar no computador. Ele vai para `settings.json` (`known`: nome, impressão digital, último IP e **posição da tela**) e o motor passa a aceitá-lo sem pedir o código (`Command::SetTrusted`). Nome igual com outra chave **não** é aceito: pede o código. "Esquecer" em Opções avançadas remove a confiança.
- **Posição por computador:** ao mudar a posição do outro PC no mapa, ela é salva para o computador conhecido conectado; ao reconectar (ou antes, ao conectar por descoberta) a posição salva é aplicada.
- **Conexão automática:** computadores conhecidos que aparecem na rede (ou o último IP) conectam sozinhos, sem aviso. O de menor IP toma a iniciativa; o outro entra depois de ~8 s. O vigia sonda a porta (1,2 s) antes de conectar por IP salvo, para não travar em "Conectando…" com o outro PC desligado.
- **Descoberta em várias redes:** os anúncios saem por cada interface IPv4 da LAN usando a máscara real (antes assumia /24, o que falha em redes /23 como 10.0.254.x ↔ 10.0.255.x) e há diagnóstico em Opções avançadas (interfaces e anúncios recebidos).

### 0.3.8 — endereço correto em PCs com várias placas

- O IP mostrado ("Endereço deste PC") agora é o da placa real que sai pela rota padrão; placas virtuais (Hyper-V/WSL, VMware, VirtualBox, Docker, VPN) ficam por último. Os demais endereços da LAN aparecem como alternativa ("Se não funcionar, tente: …").

### 0.4.0 — painel

- Interface refeita como painel (abas **Painel / Conectar / Ajustes**), sem rolagem da janela (1040×680, mínimo 980×640): cartão de conexão em degradê, estatísticas, mapa de posição das telas, atividade, computadores e guia rápido, com ícones SVG próprios (`Icons.tsx`) e logotipo em SVG. Estilos em `dashboard.css`.

### 0.5.0 — pastas, abrir da bandeja, troca de tela livre

- **Pastas:** arrastar uma pasta até a borda envia todos os arquivos dentro dela, recursivamente (`file_transfer::plan`). Protocolo v4: `FileOffer` ganhou `rel` (caminho relativo com `/`, validado componente a componente), `root_bytes` e `root_last`. No destino, a pasta é recriada em Downloads (`Pasta (1)` se já existe; nada é sobrescrito) e a bandeja mostra **um item por pasta**. Atalhos, links e junções não são seguidos; pastas vazias não são recriadas; limites: 10 000 arquivos e 64 GiB por lote, 32 níveis.
- **Bandeja:** clique duplo no item abre o arquivo/pasta com o programa padrão (`ShellExecute`); o botão de pasta abre o Explorer com o item selecionado.
- **Troca de tela:** só fica travada enquanto o botão do mouse está pressionado (segurando o arquivo na borda). Durante o envio/recebimento (já com o botão solto) o cursor cruza normalmente.
