//! SQLite adapter — implements every domain port (`crate::domain::ports`)
//! on the existing `GrugDb`. Each submodule hosts one `impl PortName for
//! GrugDb` block. HTTP-shaped methods contain data-access logic directly;
//! MCP-shaped methods delegate to `crate::tools::*` utilities.
//!
//! Phase 3 migrated all HTTP `dispatch_tool` arms to call through these traits
//! and consolidated the `*_json` data-access function bodies into these impls.

pub mod brains;
pub mod config;
pub mod conversation;
pub mod docs;
pub mod dream;
pub mod graph;
pub mod memories;
pub mod recall;
pub mod search;
pub mod sync;
pub mod write;

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use crate::domain::ports::{
        BrainPort, ConfigPort, ConversationPort, DocsPort, DreamPort, GraphPort, MemoryPort,
        RecallPort, SearchPort, SyncPort, WritePort,
    };
    use crate::tools::test_helpers::test_db;
    use crate::tools::GrugDb;

    /// Compile-time proof that `GrugDb` satisfies every port from Phase 1.
    /// If any trait is missing or has an `unimplemented!()` stub that fails
    /// to type-check, this fails to build. (DW-2.2.)
    fn assert_all_ports_implemented() {
        fn requires<T>()
        where
            T: BrainPort
                + MemoryPort
                + SearchPort
                + GraphPort
                + WritePort
                + RecallPort
                + DreamPort
                + SyncPort
                + DocsPort
                + ConfigPort
                + ConversationPort,
        {
        }
        requires::<GrugDb>();
    }

    #[test]
    fn test_DW_2_1_lib_declares_adapters_module() {
        // `src/lib.rs` must include `pub mod adapters;`. We compile-include
        // the source (relative to this file) and assert the line is there.
        const LIB_SRC: &str = include_str!("../../lib.rs");
        assert!(
            LIB_SRC.contains("pub mod adapters;"),
            "DW-2.1: src/lib.rs must declare `pub mod adapters;`\nlib.rs:\n{LIB_SRC}"
        );
    }

    #[test]
    fn test_DW_2_1_sqlite_mod_declares_all_submodules() {
        // Every adapter file in the plan must be `pub mod`-declared from
        // this `mod.rs`. We add `brains` (BrainPort needs a home).
        const SQLITE_MOD_SRC: &str = include_str!("mod.rs");
        let expected = [
            "brains",
            "config",
            "docs",
            "dream",
            "graph",
            "memories",
            "recall",
            "search",
            "sync",
            "write",
        ];
        let mut missing = Vec::new();
        for m in expected {
            let needle = format!("pub mod {m};");
            if !SQLITE_MOD_SRC.contains(&needle) {
                missing.push(m);
            }
        }
        assert!(
            missing.is_empty(),
            "DW-2.1: missing `pub mod` declarations in src/adapters/sqlite/mod.rs: {missing:?}"
        );
    }

    #[test]
    fn test_DW_2_2_all_ten_ports_implemented_for_GrugDb() {
        // Bind the type-level requirement so the function is not dead code.
        let _ = assert_all_ports_implemented;
        // Smoke-call one method per trait through `&mut dyn` style (we use
        // direct calls via the type to keep object-safety out of scope) —
        // a successful build of this whole module is the actual proof.
        let (mut db, _tmp) = test_db();
        // Read-only paths that should never error on an empty brain.
        let _ = BrainPort::brains_json(&mut db).expect("BrainPort::brains_json");
        let _ = BrainPort::healthz_json(&mut db).expect("BrainPort::healthz_json");
        let _ = MemoryPort::memories_json(&mut db, None).expect("MemoryPort::memories_json");
        let _ = MemoryPort::tags_json(&mut db, None).expect("MemoryPort::tags_json");
        let _ = SearchPort::grug_search(&mut db, "anything", None).expect("SearchPort::grug_search");
        let _ = SearchPort::quickswitch_json(&mut db, "x").expect("SearchPort::quickswitch_json");
        let _ = GraphPort::graph_json(&mut db, None, None, None, None).expect("GraphPort::graph_json");
        let _ = RecallPort::grug_recall(&mut db, None, None).expect("RecallPort::grug_recall");
        let _ = RecallPort::grug_read(&mut db, None, None, None).expect("RecallPort::grug_read");
        let _ = DocsPort::grug_docs(&mut db, None, None, None).expect("DocsPort::grug_docs");
        let _ = ConfigPort::grug_config(
            &mut db, "list", None, None, None, None, None, None, None, None, None,
        )
        .expect("ConfigPort::grug_config(list)");
        // SyncPort with no git brains: should be Ok (no-op text).
        let _ = SyncPort::grug_sync(&mut db, None).expect("SyncPort::grug_sync");
        // DreamPort on an empty brain returns the "nothing to dream about" path.
        let _ = DreamPort::grug_dream(&mut db).expect("DreamPort::grug_dream");
        let _ = ConversationPort::grug_conversation(&mut db, "list", None, None, None, None)
            .expect("ConversationPort::grug_conversation(list)");
    }

    #[test]
    fn test_DW_2_2_no_unimplemented_stubs_in_adapters() {
        // Any `unimplemented!()` or `todo!()` in adapter source means a
        // method has no real body. Accept these tokens only inside string
        // literals or comments — we strip those on the simple side.
        let files: &[(&str, &str)] = &[
            ("brains.rs", include_str!("brains.rs")),
            ("config.rs", include_str!("config.rs")),
            ("conversation.rs", include_str!("conversation.rs")),
            ("docs.rs", include_str!("docs.rs")),
            ("dream.rs", include_str!("dream.rs")),
            ("graph.rs", include_str!("graph.rs")),
            ("memories.rs", include_str!("memories.rs")),
            ("recall.rs", include_str!("recall.rs")),
            ("search.rs", include_str!("search.rs")),
            ("sync.rs", include_str!("sync.rs")),
            ("write.rs", include_str!("write.rs")),
        ];
        let banned = ["unimplemented!", "todo!"];
        let mut hits = Vec::new();
        for (name, src) in files {
            for token in banned {
                if src.contains(token) {
                    hits.push(format!("{name}: contains `{token}`"));
                }
            }
        }
        assert!(
            hits.is_empty(),
            "DW-2.2: adapter files must have real impls — found stubs:\n{}",
            hits.join("\n")
        );
    }

    #[test]
    fn test_DW_2_4_no_adapter_file_exceeds_300_lines() {
        let files: &[(&str, &str)] = &[
            ("mod.rs", include_str!("mod.rs")),
            ("brains.rs", include_str!("brains.rs")),
            ("config.rs", include_str!("config.rs")),
            ("conversation.rs", include_str!("conversation.rs")),
            ("docs.rs", include_str!("docs.rs")),
            ("dream.rs", include_str!("dream.rs")),
            ("graph.rs", include_str!("graph.rs")),
            ("memories.rs", include_str!("memories.rs")),
            ("recall.rs", include_str!("recall.rs")),
            ("search.rs", include_str!("search.rs")),
            ("sync.rs", include_str!("sync.rs")),
            ("write.rs", include_str!("write.rs")),
        ];
        let mut over = Vec::new();
        for (name, src) in files {
            let lines = src.lines().count();
            if lines > 300 {
                over.push(format!("{name}: {lines} lines"));
            }
        }
        assert!(
            over.is_empty(),
            "DW-2.4: adapter files must each be ≤300 lines — over-limit:\n{}",
            over.join("\n")
        );
    }
}
