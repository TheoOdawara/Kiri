Fora de escopo (deferido de propósito)

   - Achatar a árvore de módulos single-child — descartado: briga com o ADR 0003 e o os seams do 2º
   provider / session. Mitigação cosmética opcional (pub use re-exports) fica para depois.
   - PartialToolCall → BTreeMap<u32, ToolCall> e a clareza do sandbox::resolve_create — ganho
 marginal, borra invariantes; deixar para quando incomodar.