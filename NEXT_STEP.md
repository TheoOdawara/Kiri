Fora de escopo (deferido de propósito)

   - Achatar a árvore de módulos single-child — descartado: briga com o ADR 0003 e o os seams do 2º
   provider / session. Mitigação cosmética opcional (pub use re-exports) fica para depois.
   - PartialToolCall → BTreeMap<u32, ToolCall> e a clareza do sandbox::resolve_create — ganho
  marginal, borra invariantes; deixar para quando incomodar.


Resolvido (implementado)

   - Aprovação agora mostra o comando real do terminal após a descrição
     ("Listar o diretório. Aprova executar: ls .?" em vez de "Listar '.'?").
   - Rolagem fina do histórico: ⇧↑/⇧↓ (linha), PgUp/PgDn (passo), ⇧PgUp/⇧PgDn (página),
     ^Home/^End (topo/fundo).
   - Modo auto: o gate já pulava aprovação; ao escolher "Sim, e não perguntar de novo" agora
     há um aviso deixando claro que o auto vale a partir do próximo turno (o modo é congelado
     por turno intencionalmente).
   - Tempo de pensamento em Mm Ss (≥60s); abaixo disso continua "Xs".
   - Linha de "pensando" ao vivo (💭) enquanto o turno roda; some ao terminar, deixando o chat
     normal. O reasoning continua também no transcript (dim). Custo de token: zero (já vem no SSE).
   - TUI totalmente responsiva: nada corta ao redimensionar. meta rule abrevia workspace/modelo,
     header/hint colapsam em terminal estreito, transcript e editor fazem soft-wrap por palavra,
     approval box wrapar a ação e cresce em altura, command menu trunca o blurb, layout colapsa
     header/hint em altura < 8 e só mostra a linha de thinking com altura ≥ 10.
   - Bônus (correções de plataforma que bloqueavam o gate no Windows): normalização de separador
     de caminho para "/" em missing_dirs_label/search_file (snapshot estável cross-platform) e
     is_absolute_target trata leading "/" como absoluto em qualquer SO.
