use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::types::WorkspaceEntry;
use crate::utils::normalize_git_path;

use super::helpers::resolve_workspace_root;

const MAX_WORKSPACE_SYMBOLS: usize = 20_000;
const MAX_SYMBOL_SCAN_BYTES: u64 = 1_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceCallableSymbol {
    pub(crate) path: String,
    pub(crate) symbol: String,
    pub(crate) kind: String,
    pub(crate) language: String,
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "dist" | "target" | "release-artifacts"
    )
}

fn ts_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*(?:<[^>{}\n]*>)?\s*\(",
        )
        .expect("valid ts/js function regex")
    })
}

fn ts_variable_callable_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*(?::[^=\n]+)?=\s*(?:async\s*)?(?:function\b|(?:<[^>{}\n]*>\s*)?(?:\([^)]*\)|[A-Za-z_$][A-Za-z0-9_$]*)\s*=>)",
        )
        .expect("valid ts/js variable callable regex")
    })
}

fn ts_method_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:public\s+|private\s+|protected\s+|static\s+|readonly\s+|override\s+|async\s+|get\s+|set\s+)*(#?[A-Za-z_$][A-Za-z0-9_$]*)\s*(?:<[^>{}\n]*>)?\([^;=\n]*\)\s*(?::\s*[^={\n]+)?\s*\{",
        )
        .expect("valid ts/js method regex")
    })
}

fn rust_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}\n]*>)?\s*\(([^)]*)\)",
        )
        .expect("valid rust function regex")
    })
}

fn python_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)")
            .expect("valid python function regex")
    })
}

fn go_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*func\s*(\([^)]+\)\s*)?([A-Za-z_][A-Za-z0-9_]*)\s*\(")
            .expect("valid go function regex")
    })
}

fn is_hook_name(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    matches!(
        (chars.next(), chars.next(), chars.next(), chars.next()),
        (Some('u'), Some('s'), Some('e'), Some(next)) if next.is_ascii_uppercase()
    )
}

fn is_component_name(symbol: &str, extension: &str) -> bool {
    matches!(extension, "tsx" | "jsx")
        && symbol.chars().next().is_some_and(|value| value.is_ascii_uppercase())
}

fn normalize_symbol_kind(symbol: &str, extension: &str, method_like: bool) -> &'static str {
    if method_like {
        "method"
    } else if is_hook_name(symbol) {
        "hook"
    } else if is_component_name(symbol, extension) {
        "component"
    } else {
        "function"
    }
}

fn is_ignored_ts_method(symbol: &str) -> bool {
    matches!(
        symbol,
        "if"
            | "for"
            | "while"
            | "switch"
            | "catch"
            | "function"
            | "constructor"
            | "else"
            | "do"
            | "try"
    )
}

fn supported_language(path: &Path) -> Option<(&'static str, &'static str)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let value = match ext.as_str() {
        "ts" => ("ts", "typescript"),
        "tsx" => ("tsx", "typescript"),
        "mts" => ("mts", "typescript"),
        "cts" => ("cts", "typescript"),
        "js" => ("js", "javascript"),
        "jsx" => ("jsx", "javascript"),
        "mjs" => ("mjs", "javascript"),
        "cjs" => ("cjs", "javascript"),
        "rs" => ("rs", "rust"),
        "py" => ("py", "python"),
        "go" => ("go", "go"),
        _ => return None,
    };
    Some(value)
}

fn push_symbol(
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
    path: &str,
    symbol: &str,
    kind: &str,
    language: &str,
) {
    if symbol.is_empty() {
        return;
    }
    let key = (path.to_string(), symbol.to_string());
    if !seen.insert(key) {
        return;
    }
    results.push(WorkspaceCallableSymbol {
        path: path.to_string(),
        symbol: symbol.to_string(),
        kind: kind.to_string(),
        language: language.to_string(),
    });
}

fn extract_ts_callables(
    content: &str,
    path: &str,
    extension: &str,
    language: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in ts_function_regex().captures_iter(content) {
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                language,
            );
        }
    }
    for captures in ts_variable_callable_regex().captures_iter(content) {
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                language,
            );
        }
    }
    for line in content.lines() {
        if line.contains("=>") || line.contains("function") {
            continue;
        }
        if let Some(captures) = ts_method_regex().captures(line) {
            let Some(symbol) = captures.get(1) else {
                continue;
            };
            let normalized = symbol.as_str().trim_start_matches('#');
            if !is_ignored_ts_method(normalized) {
                push_symbol(
                    results,
                    seen,
                    path,
                    normalized,
                    normalize_symbol_kind(normalized, extension, true),
                    language,
                );
            }
        }
    }
}

fn extract_rust_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in rust_function_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let params = captures.get(2).map(|value| value.as_str()).unwrap_or_default();
        let method_like = params.contains("self");
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, method_like),
            "rust",
        );
    }
}

fn extract_python_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in python_function_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let params = captures.get(2).map(|value| value.as_str()).unwrap_or_default();
        let first_param = params.split(',').next().map(str::trim).unwrap_or_default();
        let method_like = matches!(first_param, "self" | "cls");
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, method_like),
            "python",
        );
    }
}

fn extract_go_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in go_function_regex().captures_iter(content) {
        let Some(symbol) = captures.get(2) else {
            continue;
        };
        let method_like = captures
            .get(1)
            .map(|value| !value.as_str().trim().is_empty())
            .unwrap_or(false);
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, method_like),
            "go",
        );
    }
}

fn extract_callables_for_file(
    content: &str,
    path: &str,
    extension: &str,
    language: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    match language {
        "typescript" | "javascript" => {
            extract_ts_callables(content, path, extension, language, results, seen)
        }
        "rust" => extract_rust_callables(content, path, extension, results, seen),
        "python" => extract_python_callables(content, path, extension, results, seen),
        "go" => extract_go_callables(content, path, extension, results, seen),
        _ => {}
    }
}

fn list_workspace_callable_symbols(root: &PathBuf, max_symbols: usize) -> Vec<WorkspaceCallableSymbol> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .follow_links(false)
        .require_git(false)
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                let name = entry.file_name().to_string_lossy();
                return !should_skip_dir(&name);
            }
            true
        })
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let Some((extension, language)) = supported_language(entry.path()) else {
            continue;
        };
        let Ok(relative_path) = entry.path().strip_prefix(root) else {
            continue;
        };
        let normalized_path = normalize_git_path(&relative_path.to_string_lossy());
        if normalized_path.is_empty() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > MAX_SYMBOL_SCAN_BYTES {
            continue;
        }
        let Ok(content) = fs::read_to_string(entry.path()) else {
            continue;
        };
        extract_callables_for_file(
            &content,
            &normalized_path,
            extension,
            language,
            &mut results,
            &mut seen,
        );
        if results.len() >= max_symbols {
            break;
        }
    }

    results.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.symbol.cmp(&right.symbol))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    results.truncate(max_symbols);
    results
}

pub(crate) async fn list_workspace_symbols_core(
    workspaces: &Mutex<HashMap<String, WorkspaceEntry>>,
    workspace_id: &str,
) -> Result<Vec<WorkspaceCallableSymbol>, String> {
    let root = resolve_workspace_root(workspaces, workspace_id).await?;
    Ok(list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use crate::types::{WorkspaceKind, WorkspaceSettings};

    fn temp_workspace_root(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("valid time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("codex-monitor-symbols-{name}-{unique}"));
        fs::create_dir_all(&path).expect("create temp workspace");
        path
    }

    fn write_file(root: &Path, path: &str, content: &str) {
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(target, content).expect("write file");
    }

    #[test]
    fn extracts_supported_language_callables() {
        let root = temp_workspace_root("multi");
        write_file(
            &root,
            "src/components/App.tsx",
            r#"
export function useAlpha() {}
export const Button = () => null;
class Widget {
  renderThing() {}
}
"#,
        );
        write_file(
            &root,
            "src/lib.rs",
            r#"
pub fn build() {}
impl Worker {
    pub fn run(&self) {}
}
"#,
        );
        write_file(
            &root,
            "scripts/main.py",
            r#"
def helper():
    pass

class Runner:
    def execute(self):
        pass
"#,
        );
        write_file(
            &root,
            "pkg/service.go",
            r#"
func Build() {}
func (s *Server) Run() {}
"#,
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| format!("{}:{}:{}", entry.path, entry.symbol, entry.kind))
            .collect::<Vec<_>>();

        assert!(keys.contains(&"pkg/service.go:Build:function".to_string()));
        assert!(keys.contains(&"pkg/service.go:Run:method".to_string()));
        assert!(keys.contains(&"scripts/main.py:execute:method".to_string()));
        assert!(keys.contains(&"scripts/main.py:helper:function".to_string()));
        assert!(keys.contains(&"src/components/App.tsx:Button:component".to_string()));
        assert!(keys.contains(&"src/components/App.tsx:renderThing:method".to_string()));
        assert!(keys.contains(&"src/components/App.tsx:useAlpha:hook".to_string()));
        assert!(keys.contains(&"src/lib.rs:build:function".to_string()));
        assert!(keys.contains(&"src/lib.rs:run:method".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skips_ignored_dirs_and_deduplicates_symbols() {
        let root = temp_workspace_root("ignored");
        write_file(
            &root,
            "src/feature.ts",
            r#"
export function duplicate() {}
export const duplicate = () => {};
"#,
        );
        write_file(&root, "node_modules/pkg/index.ts", "export function ignored() {}");
        write_file(&root, "dist/build.ts", "export function ignoredBuild() {}");

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| format!("{}:{}", entry.path, entry.symbol))
            .collect::<Vec<_>>();

        assert_eq!(keys, vec!["src/feature.ts:duplicate".to_string()]);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scopes_symbol_listing_to_the_requested_workspace() {
        let root_a = temp_workspace_root("workspace-a");
        let root_b = temp_workspace_root("workspace-b");
        write_file(&root_a, "src/a.ts", "export function alpha() {}");
        write_file(&root_b, "src/b.ts", "export function beta() {}");

        let mut workspaces = HashMap::new();
        workspaces.insert(
            "ws-a".to_string(),
            WorkspaceEntry {
                id: "ws-a".to_string(),
                name: "Workspace A".to_string(),
                path: root_a.to_string_lossy().to_string(),
                kind: WorkspaceKind::Main,
                parent_id: None,
                worktree: None,
                settings: WorkspaceSettings::default(),
            },
        );
        workspaces.insert(
            "ws-b".to_string(),
            WorkspaceEntry {
                id: "ws-b".to_string(),
                name: "Workspace B".to_string(),
                path: root_b.to_string_lossy().to_string(),
                kind: WorkspaceKind::Main,
                parent_id: None,
                worktree: None,
                settings: WorkspaceSettings::default(),
            },
        );
        let workspaces = Mutex::new(workspaces);

        let runtime = tokio::runtime::Runtime::new().expect("create tokio runtime");
        let symbols = runtime
            .block_on(list_workspace_symbols_core(&workspaces, "ws-b"))
            .expect("list symbols");

        assert_eq!(
            symbols,
            vec![WorkspaceCallableSymbol {
                path: "src/b.ts".to_string(),
                symbol: "beta".to_string(),
                kind: "function".to_string(),
                language: "typescript".to_string(),
            }]
        );

        let _ = fs::remove_dir_all(root_a);
        let _ = fs::remove_dir_all(root_b);
    }
}
