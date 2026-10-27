# Segurança e riscos Windows

## Princípios (do plano)

Consentimento explícito por pareamento; identidade criptográfica por dispositivo; transporte autenticado e criptografado;
nenhum conteúdo de tecla em logs, telemetria ou disco; limites de tamanho em toda entrada de rede
(já aplicados em `protocol::decode`); caminhos de arquivo recebidos nunca confiáveis (`file-transfer::is_safe_relative`).

## Riscos técnicos Windows

| Risco                                      | Impacto                                                                                     | Mitigação                                                         |
| ------------------------------------------ | ------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| Hooks globais WH_*_LL                      | Hook lento ou thread travada congela o input do sistema; Windows remove o hook após timeout | Thread dedicada com message loop, callback mínimo, watchdog       |
| Raw Input                                  | Observa, mas não suprime                                                                    | Usar só para observação; supressão via hook LL                    |
| UIPI                                       | SendInput não atinge processos de integridade superior (apps elevados, UAC)                 | Documentar limitação; não contornar                               |
| Sessão elevada / secure desktop / bloqueio | Não há input remoto em UAC ou tela de login                                                 | Detectar bloqueio → RECOVERING                                    |
| Multi-monitor / DPI                        | Coordenadas erradas entre PCs com escalas distintas                                         | Coordenadas normalizadas por monitor; testar DPI misto            |
| Sleep / troca de rede                      | Controle preso no remoto                                                                    | Heartbeat (timeout 1,5 s), eventos de energia/sessão → RECOVERING |
| Teclas presas no receptor                  | Modificador ficando pressionado                                                             | `release_all` em RECOVERING                                       |
| Jogos/fullscreen exclusivo                 | Podem ignorar SendInput/hooks                                                               | Limitação documentada                                             |
