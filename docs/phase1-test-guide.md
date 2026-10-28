# Guia de teste da Fase 1 (dois PCs)

Instaladores: `target\release\bundle\nsis\Windows Flow_0.6.0_x64-setup.exe` (gerar com `npm run build`).
Rede: os dois PCs na mesma LAN. Ao escutar pela primeira vez, o Windows pede acesso no Firewall: permitir **somente rede privada**.

## Antes de começar

- Saiba o atalho de emergência: **Ctrl+Alt+Shift+F12** (devolve mouse e teclado ao PC onde for pressionado).
- Tenha um mouse/teclado extra à mão no primeiro teste, por precaução.

## Passo a passo

1. PC A: abrir o app → "Aguardar conexão neste PC". Anotar o IP exibido.
2. PC B: abrir o app → digitar o IP de A → "Conectar".
3. Os dois mostram um código de 6 dígitos. Se forem **iguais**, clicar "Os códigos são iguais" nos dois.
4. Em cada PC, escolher onde está o **outro** (ex.: B à direita de A → A: "direita", B: "esquerda").
5. Encostar o cursor na borda de A voltada para B e segurar ~0,1 s.

## Checklist (marcar com data e resultado real)

| #   | Teste                                                       | Esperado                                                | Resultado |
| --- | ----------------------------------------------------------- | ------------------------------------------------------- | --------- |
| 1   | Códigos iguais nos dois PCs                                 | iguais                                                  |           |
| 2   | Cruzar A→B                                                  | cursor aparece em B na mesma altura                     |           |
| 3   | Voltar B→A (empurrar contra a borda)                        | cursor volta a A, teclado volta a A                     |           |
| 4   | Digitar texto em B                                          | aparece só em B, sem duplicar em A                      |           |
| 5   | Clique, arrastar, rolagem vertical/horizontal               | funcionam em B                                          |           |
| 6   | Botões laterais do mouse (X1/X2)                            | funcionam em B                                          |           |
| 7   | Atalhos (Ctrl+C/V, Alt+Tab) em B                            | executam em B                                           |           |
| 8   | Teclas especiais, acentos, Win, setas, Fn                   | ver limitação de layout                                 |           |
| 9   | Segurar Shift ao cruzar, soltar em B                        | Shift não fica preso em A nem em B                      |           |
| 10  | Tirar o cabo/Wi-Fi de B com A em REMOTE                     | A volta a LOCAL em ≤ 2 s                                |           |
| 11  | Fechar o app em B com A em REMOTE                           | A volta a LOCAL na hora                                 |           |
| 12  | Atalho de emergência em A (REMOTE) com a rede **desligada** | A volta a LOCAL na hora                                 |           |
| 13  | Atalho de emergência em B (controlado)                      | B solta tudo; A volta a LOCAL                           |           |
| 14  | Reconexão: reconectar a rede depois do item 10              | reconecta sem pedir código novo (app do cliente aberto) |           |
| 15  | Monitores com DPI/escala diferentes                         | posição de entrada coerente                             |           |
| 16  | Janela elevada (Executar como admin) em B                   | **não** recebe entrada (limitação UIPI)                 |           |
| 17  | Jogo/vídeo em tela cheia                                    | anotar comportamento                                    |           |
| 18  | CPU do app parado e conectado (Gerenciador de Tarefas)      | anotar %                                                |           |

## Como relatar problemas

Anotar: passo, o que aconteceu, o que esperava, versão do Windows dos dois PCs, tipo de rede (Wi-Fi/Ethernet).
Se mouse/teclado ficarem presos: Ctrl+Alt+Shift+F12; se não resolver, Ctrl+Alt+Del e encerrar o Windows Flow.
