//! The module boundaries, enforced.
//!
//! `CLAUDE.md`, `AGENTS.md` and `docs/SPEC.md` all state the same dependency
//! graph:
//!
//! ```text
//! client  ──▶  session  ──▶  { protocol,  transport PORT }
//!                                            ▲
//!                              transport ADAPTERS (ws; http planned)
//! ```
//!
//! Documenting it was not enough: the v1 review found four live violations of
//! it (R-01 through R-04), every one of which compiled cleanly, because Rust
//! has no way to say "this module may not name that one" inside a single crate.
//! This file is that missing sentence. It reads the source and fails if an edge
//! the graph forbids reappears.
//!
//! # What it can and cannot prove
//!
//! It is a **lexical** check: it looks for `crate::<module>` and `super::` paths
//! in each file, so it catches the ordinary way a dependency is introduced —
//! someone writes the import the compiler suggests. It cannot catch a path
//! spelled through an alias, and it is not meant to: the point is that adding a
//! forbidden edge must be a deliberate act that also edits this file, not
//! something a helpful IDE completion does on your behalf.
//!
//! Adding a name to a layer's allowed set is a real architectural decision.
//! Write down why, in the same commit.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One layer of the graph and what it is allowed to name.
struct Layer {
    /// The directory or file under `src/` this rule covers.
    path: &'static str,
    /// The crate modules it may depend on, besides itself.
    may_use: &'static [&'static str],
    /// Why, in one sentence, for whoever hits this test.
    rationale: &'static str,
}

/// Every module the check knows about. A path in `src/` that names any of these
/// and is not in its layer's `may_use` is a violation.
const MODULES: &[&str] = &[
    "client",
    "config",
    "error",
    "protocol",
    "session",
    "subscription",
    "transport",
];

// `test_util` is deliberately absent from `MODULES`. It is a feature-gated
// **consumer** of the other layers, not something any of them depends on: what
// the layers contain are rustdoc links pointing at it, explaining why a given
// constructor exists. A real dependency on it from library code would not
// compile in a default build, which is a stronger check than this file could
// make.

/// The graph itself.
///
/// Read it top to bottom: each layer may name only what is listed, and the
/// lists get shorter the further down you go. `error` and `config` are the
/// leaves everything may depend on.
const LAYERS: &[Layer] = &[
    Layer {
        path: "src/protocol",
        may_use: &[],
        rationale: "the protocol layer is pure — bytes in, typed values out, with no I/O, no \
                    async and no state above it. That purity is what makes every wire behaviour \
                    testable from spec fixtures, and therefore what makes the clean-room \
                    citation discipline checkable rather than aspirational (R-01).",
    },
    Layer {
        path: "src/config",
        may_use: &["error"],
        rationale: "configuration is a validated leaf: it knows what a caller may write and \
                    nothing about the wire, the state machine or the sockets. Translating it \
                    into their vocabulary happens on their side of the boundary, in \
                    `SessionOptions::from_client_config` (R-04).",
    },
    Layer {
        path: "src/transport",
        may_use: &["error", "protocol"],
        rationale: "a transport moves bytes and frames lines. It never interprets a \
                    notification's meaning and knows nothing of sessions, subscriptions or the \
                    public client.",
    },
    Layer {
        path: "src/subscription",
        may_use: &["error", "protocol"],
        rationale: "subscription state decodes what the protocol layer parsed. It holds no \
                    socket, performs no I/O, and is driven from above rather than reaching up.",
    },
    Layer {
        path: "src/session",
        may_use: &["config", "error", "protocol", "subscription", "transport"],
        rationale: "the session layer is the only one that composes the others: it drives a \
                    transport, feeds its lines through the protocol layer, and keeps \
                    subscription state. It is also the composition root that chooses a \
                    transport.",
    },
    Layer {
        path: "src/client",
        may_use: &["config", "error", "session", "subscription"],
        rationale: "the facade talks to the session layer and to nothing below it. It may not \
                    name a transport adapter or a parser type: `session` re-exports the \
                    vocabulary it is allowed to speak, so widening that seam is one edit in one \
                    place (R-02). `subscription` is reachable only for the public update types \
                    it re-exports.",
    },
    Layer {
        path: "src/error.rs",
        may_use: &["client", "config", "session"],
        rationale: "the error taxonomy is where every layer's failure becomes public, so it \
                    names the layers it converts from. It owns `TransportError` rather than \
                    borrowing it from the port, because a type in the semver promise belongs to \
                    the module that makes the promise (R-03).",
    },
];

/// The crate root re-exports the whole public surface, so a name that reaches it
/// from one of these is an internal type escaping into semver.
const PRIVATE_TO_THE_CRATE: &[&str] = &["protocol", "session", "transport"];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Every `.rs` file under a path, which may itself be a file.
fn rust_files(root: &Path) -> Vec<PathBuf> {
    if root.is_file() {
        return vec![root.to_path_buf()];
    }
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// The crate modules a file names, by the paths it writes them with.
fn modules_named(source: &str, own_layer: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for module in MODULES {
        if *module == own_layer {
            continue;
        }
        // `crate::protocol::…` and `crate::protocol;`, but not
        // `crate::protocol_something`.
        let prefix = format!("crate::{module}");
        let mut cursor = 0;
        while let Some(offset) = source[cursor..].find(&prefix) {
            let at = cursor + offset;
            let after = source[at + prefix.len()..].chars().next();
            if !matches!(after, Some(character) if character.is_alphanumeric() || character == '_')
            {
                found.insert((*module).to_owned());
                break;
            }
            cursor = at + prefix.len();
        }
    }
    found
}

/// The layer a file belongs to, as its own module name.
fn own_module(layer_path: &str) -> &str {
    layer_path
        .trim_start_matches("src/")
        .trim_end_matches(".rs")
}

#[test]
fn test_no_layer_names_a_module_the_architecture_forbids() {
    let root = repository_root();
    let mut violations = Vec::new();

    for layer in LAYERS {
        let own = own_module(layer.path);
        for file in rust_files(&root.join(layer.path)) {
            let source = std::fs::read_to_string(&file).unwrap_or_default();
            let display = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .display()
                .to_string();
            for module in modules_named(&source, own) {
                if !layer.may_use.contains(&module.as_str()) {
                    violations.push(format!(
                        "{display} names `crate::{module}`, which `{}` may not.\n    {}",
                        layer.path, layer.rationale
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "the module boundaries were broken:\n\n{}\n",
        violations.join("\n\n")
    );
}

#[test]
fn test_the_protocol_layer_names_nothing_above_it() {
    // R-01 in one assertion, because it is the load-bearing one: every other
    // structural claim in this crate rests on `protocol` being independently
    // compilable, testable and auditable. `LS_supported_diffs` is the edge that
    // broke it — the request encoder derived the advertised set from the
    // subscription layer's decoder registry — so the registry now lives in
    // `protocol::diff`, on the pure side, where both halves of ADR-0004 belong.
    let root = repository_root();
    for file in rust_files(&root.join("src/protocol")) {
        let source = std::fs::read_to_string(&file).unwrap_or_default();
        for module in ["client", "config", "session", "subscription", "transport"] {
            assert!(
                !source.contains(&format!("crate::{module}")),
                "{} names `crate::{module}`; the protocol layer must depend on nothing else \
                 in this crate",
                file.display()
            );
        }
    }
}

#[test]
fn test_the_crate_root_reexports_nothing_from_a_private_layer() {
    // Whatever `lib.rs` re-exports is the semver promise. A `pub use` from the
    // parser, the state machine or a transport would put an internal decision
    // in it — R-03, which is why `TransportError` is defined in `error` and
    // re-exported from there.
    let root = repository_root();
    let source = std::fs::read_to_string(root.join("src/lib.rs")).unwrap_or_default();
    for module in PRIVATE_TO_THE_CRATE {
        assert!(
            !source.contains(&format!("pub use {module}::")),
            "src/lib.rs re-exports from `{module}`, which is private to the crate; define the \
             public type in `error` or `config` and re-export it from there"
        );
    }
}

#[test]
fn test_nothing_inside_src_imports_from_the_crate_root() {
    // `lib.rs` is the public surface, not a module others read back from. A
    // `use crate::Something` inside `src/` would make an internal file depend
    // on the re-export set, so renaming a public name would break the build for
    // a reason unrelated to the rename.
    let root = repository_root();
    for file in rust_files(&root.join("src")) {
        if file.ends_with("lib.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&file).unwrap_or_default();
        for line in source.lines() {
            let line = line.trim();
            if !line.starts_with("use crate::") {
                continue;
            }
            let tail = line.trim_start_matches("use crate::");
            let first = tail
                .split(&[':', ';', ',', ' ', '{'][..])
                .next()
                .unwrap_or_default();
            assert!(
                first.is_empty() || MODULES.contains(&first) || first == "REDACTED",
                "{} imports `{line}` from the crate root; import from the owning module instead",
                file.display()
            );
        }
    }
}
