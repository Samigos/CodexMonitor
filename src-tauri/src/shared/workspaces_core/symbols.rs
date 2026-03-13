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
    let normalized = name.to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        ".git"
            | "node_modules"
            | "dist"
            | "target"
            | "release-artifacts"
            | "build"
            | "pods"
            | "vendor"
            | ".gradle"
            | ".dart_tool"
            | ".build"
            | "carthage"
            | "deriveddata"
            | "_build"
            | "dist-newstyle"
            | ".stack-work"
            | ".venv"
            | "venv"
            | "site-packages"
            | "__pycache__"
            | ".tox"
            | ".nox"
            | ".mypy_cache"
            | ".pytest_cache"
            | "bazel-bin"
            | "bazel-out"
            | "bazel-testlogs"
            | "buck-out"
    ) || normalized.starts_with("cmake-build-")
}

fn should_skip_generated_symbol_file(path: &Path) -> bool {
    if is_dotnet_obj_source_file(path) {
        return true;
    }
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let normalized = name.to_ascii_lowercase();
    normalized.ends_with(".g.dart")
        || normalized.ends_with(".pb.go")
        || normalized.ends_with("_pb2.py")
        || normalized.ends_with(".designer.cs")
        || normalized.ends_with(".generated.cs")
        || normalized.ends_with(".g.cs")
        || normalized.ends_with(".g.i.cs")
        || normalized.ends_with(".designer.vb")
        || normalized.ends_with(".generated.vb")
        || normalized.ends_with(".g.vb")
        || normalized.ends_with(".g.i.vb")
}

fn is_dotnet_obj_source_file(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    if !matches!(extension.to_ascii_lowercase().as_str(), "cs" | "vb") {
        return false;
    }
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|value| value.eq_ignore_ascii_case("obj"))
    })
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

fn c_family_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"^\s*(?:(?:public|private|protected|internal|static|final|abstract|virtual|override|async|sealed|partial|native|synchronized|extern|inline|constexpr|friend|explicit|unsafe|new|typedef|signed|unsigned|short|long)\s+)*(?:[A-Za-z_(][A-Za-z0-9_:.<>\[\],\s\*&?~()]*?)\s*(?:[*&]+\s*)?([A-Za-z_~][A-Za-z0-9_:~]*)(?:\s*<[^;=(){}\n<>]*(?:<[^;=(){}\n<>]+>[^;=(){}\n<>]*)*>)?\s*\([^;=\n]*\)\s*(?:(?:const|volatile|override|final)\s*|(?:noexcept(?:\([^)]*\))?)\s*|(?:throws\s+[^{;\n]+)\s*|(?:where\s+[^={;\n]+(?:\([^)]*\))?)\s*|(?:requires\s+[^={;\n]+(?:\([^)]*\))?)\s*|(?:->\s*[^={;\n]+)\s*|&&\s*|&\s*)*(?:\{|;|=>)",
        )
        .expect("valid c-family function regex")
    })
}

fn c_family_constructor_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"^\s*(?:(?:public|private|protected|internal|static|final|abstract|virtual|override|async|sealed|partial|native|synchronized|extern|inline|constexpr|friend|explicit|unsafe|new)\s+)*((?:~?[A-Za-z_][A-Za-z0-9_]*)(?:::(?:~?[A-Za-z_][A-Za-z0-9_]*))*)(?:\s*<[^;=(){}\n<>]*(?:<[^;=(){}\n<>]+>[^;=(){}\n<>]*)*>)?\s*\([^;=\n]*\)\s*(?:const\s*)?(?:noexcept(?:\([^)]*\))?\s*)?(?:\:\s*[^;{\n]+)?(?:throws\s+[^{;\n]+)?(?:where\s+[^={;\n]+(?:\([^)]*\))?\s*)*(?:\{|;|=>)",
        )
        .expect("valid c-family constructor regex")
    })
}

fn php_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:(?:public|private|protected|static|final|abstract)\s+)*function\s+&?\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(",
        )
        .expect("valid php function regex")
    })
}

fn ruby_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*def\s+(?:self\.|[A-Z][A-Za-z0-9_:]*\.)?([A-Za-z_][A-Za-z0-9_]*[!?=]?)")
            .expect("valid ruby function regex")
    })
}

fn swift_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:@[_A-Za-z][_A-Za-z0-9.]*(?:\([^)\n]*\))?\s+)*(?:(?:public|private|fileprivate|internal|open|final|static|class|mutating|nonmutating|override|convenience|required)\s+)*func\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}\n]*>)?\s*\(",
        )
        .expect("valid swift function regex")
    })
}

fn kotlin_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:(?:@[_A-Za-z][_A-Za-z0-9.]*(?::[_A-Za-z][_A-Za-z0-9.]*)?(?:\([^)\n]*\))?|public|private|protected|internal|open|final|abstract|override|suspend|inline|tailrec|operator|infix|external)\s+)*fun\s+(?:<[^>{}\n]*>\s*)?(?:(?:[A-Za-z_][A-Za-z0-9_<>,?. \t]*|\([^)\n]+\))\.)?([A-Za-z_][A-Za-z0-9_]*)\s*\(",
        )
        .expect("valid kotlin function regex")
    })
}

fn scala_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:(?:private|protected|override|final|abstract|sealed|implicit|lazy|case|inline)\s+)*def\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:\[[^\]\n]*\])?\s*(?:\([^=\n]*\))?\s*(?::\s*[^{=\n]+)?\s*(?:=|\{)",
        )
        .expect("valid scala function regex")
    })
}

fn dart_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"^\s*(?:(?:external|static|const|factory|covariant|final|abstract)\s+)*(?:[A-Za-z_(][A-Za-z0-9_<>\[\]\?,{}\s.():]*?\s+)?([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)?)\s*\([^;=\n]*\)\s*(?:async\s*)?(?::\s*[^;{\n]+)?\s*(?:\{|=>)",
        )
        .expect("valid dart function regex")
    })
}

fn lua_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:local\s+)?function\s+(?:[A-Za-z_][A-Za-z0-9_]*[:.])?([A-Za-z_][A-Za-z0-9_]*)\s*\(",
        )
        .expect("valid lua function regex")
    })
}

fn shell_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:function\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:\(\))?\s*\{|([A-Za-z_][A-Za-z0-9_]*)\s*\(\)\s*\{)",
        )
        .expect("valid shell function regex")
    })
}

fn powershell_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?im)^\s*function\s+([A-Za-z_][A-Za-z0-9_-]*)\b")
            .expect("valid powershell function regex")
    })
}

fn perl_sub_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*sub\s+([A-Za-z_][A-Za-z0-9_]*)\b").expect("valid perl sub regex")
    })
}

fn r_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*([A-Za-z.][A-Za-z0-9._]*)\s*(?:<-|=)\s*function\s*\(")
            .expect("valid r function regex")
    })
}

fn julia_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:function\s+([A-Za-z_][A-Za-z0-9_!]*)\s*(?:\{[^}\n]*\})?\s*\(|([A-Za-z_][A-Za-z0-9_!]*)\s*\([^=\n]*\)\s*=)",
        )
        .expect("valid julia function regex")
    })
}

fn elixir_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*defp?\s+([a-z_][A-Za-z0-9_!?]*)\s*(?:\(|do\b)")
            .expect("valid elixir function regex")
    })
}

fn erlang_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*([a-z][A-Za-z0-9_]*)\s*\([^)]*\)\s*->")
            .expect("valid erlang function regex")
    })
}

fn groovy_def_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(")
            .expect("valid groovy function regex")
    })
}

fn clojure_defn_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*\(defn-?\s+([A-Za-z_*+!?.<>/-][A-Za-z0-9_*+!?.<>/-]*)")
            .expect("valid clojure function regex")
    })
}

fn objective_c_method_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?ms)^\s*[+-]\s*\([^)\n]+\)\s*([^;{]+?)(?:;|\{)")
            .expect("valid objective-c method regex")
    })
}

fn c_family_type_declaration_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"^\s*(?:(?:public|private|protected|internal|static|final|abstract|sealed|partial|export|readonly)\s+)*(?:class|struct|interface|enum|union|record(?:\s+(?:class|struct))?)\s+([A-Za-z_][A-Za-z0-9_]*)([^\n]*)$",
        )
        .expect("valid c-family type declaration regex")
    })
}

fn objective_c_selector_part_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\s*:").expect("valid objective-c selector regex")
    })
}

fn haskell_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^([a-z_][A-Za-z0-9_']*)\b(?:\s+[^=\n]+)?=")
            .expect("valid haskell function regex")
    })
}

fn ocaml_let_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*let\s+(?:rec\s+)?([a-z_][A-Za-z0-9_']*)\b([^\n=]*)=\s*([^\n]*)")
            .expect("valid ocaml let regex")
    })
}

fn fsharp_let_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*let\s+(?:mutable\s+)?(?:rec\s+)?([A-Za-z_][A-Za-z0-9_']*)\b([^\n=]*)=\s*([^\n]*)",
        )
        .expect("valid fsharp let regex")
    })
}

fn fsharp_member_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:static\s+)?member\s+(?:[A-Za-z_][A-Za-z0-9_']*\.)?([A-Za-z_][A-Za-z0-9_']*)\b([^\n=]*)=\s*([^\n]*)",
        )
        .expect("valid fsharp member regex")
    })
}

fn vb_callable_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?im)^\s*(?:(?:public|private|protected|friend|shared|static|async|overrides|overridable|mustoverride|notoverridable|partial)\s+)*(?:function|sub)\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:\(|$)",
        )
        .expect("valid vb function regex")
    })
}

fn pascal_callable_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?im)^\s*(?:(?:class\s+)?(?:function|procedure|operator|constructor|destructor))\s+([A-Za-z_][A-Za-z0-9_.]*)\b",
        )
        .expect("valid pascal callable regex")
    })
}

fn assembly_proc_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?im)^\s*([A-Za-z_@$?.][A-Za-z0-9_@$?.]*)\s+proc\b")
            .expect("valid assembly proc regex")
    })
}

fn assembly_label_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*([A-Za-z_@$?.][A-Za-z0-9_@$?.]*)\s*:\s*(?:[#;].*)?$")
            .expect("valid assembly label regex")
    })
}

fn sql_callable_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?im)^\s*create\s+(?:or\s+replace\s+)?(?:function|procedure)\s+((?:[A-Za-z_][A-Za-z0-9_$]*\.)*[A-Za-z_][A-Za-z0-9_$]*)\b",
        )
        .expect("valid sql callable regex")
    })
}

fn matlab_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*function\s+(?:\[[^\]]+\]\s*=|[A-Za-z_][A-Za-z0-9_]*\s*=)?\s*([A-Za-z_][A-Za-z0-9_]*)\s*(?:\(|$)",
        )
        .expect("valid matlab function regex")
    })
}

fn sas_macro_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?im)^\s*%macro\s+([A-Za-z_][A-Za-z0-9_]*)\b").expect("valid sas macro regex")
    })
}

fn fortran_callable_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?im)^\s*(?:(?:recursive|pure|elemental|module)\s+)*(?:subroutine|function)\s+([A-Za-z_][A-Za-z0-9_]*)\b",
        )
        .expect("valid fortran callable regex")
    })
}

fn cobol_function_id_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?im)^\s*function-id\.\s*([A-Za-z0-9-]+)\s*\.")
            .expect("valid cobol function-id regex")
    })
}

fn cobol_entry_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?im)^\s*entry\s+"?([A-Za-z0-9-]+)"?\s*\."#).expect("valid cobol entry regex")
    })
}

fn cobol_paragraph_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?im)^\s{0,7}([A-Za-z0-9-]+)\.\s*$").expect("valid cobol paragraph regex")
    })
}

fn common_lisp_defun_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*\(defun\s+([A-Za-z*+!?.<>/-][A-Za-z0-9*+!?.<>/-]*)")
            .expect("valid common lisp defun regex")
    })
}

fn scheme_define_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*\(define\s+\(([A-Za-z*+!?.<>/-][A-Za-z0-9*+!?.<>/-]*)\b")
            .expect("valid scheme define regex")
    })
}

fn scheme_lambda_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*\(define\s+([A-Za-z*+!?.<>/-][A-Za-z0-9*+!?.<>/-]*)\s+\(lambda\b")
            .expect("valid scheme lambda regex")
    })
}

fn tcl_proc_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?im)^\s*proc\s+([A-Za-z_:][A-Za-z0-9_:]*)\s+\{")
            .expect("valid tcl proc regex")
    })
}

fn prolog_predicate_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*([a-z][A-Za-z0-9_]*)\s*(?:\([^)]*\))?\s*(?::-|-->|\.)(?:\s|$)")
            .expect("valid prolog predicate regex")
    })
}

fn nim_callable_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:proc|func|method|template|macro|iterator)\s+([A-Za-z_][A-Za-z0-9_]*)\*?(?:\[[^\]\n]*\])?\s*\(",
        )
        .expect("valid nim callable regex")
    })
}

fn zig_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*(?:pub\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(")
            .expect("valid zig function regex")
    })
}

fn solidity_function_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*function\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(")
            .expect("valid solidity function regex")
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
        && symbol
            .chars()
            .next()
            .is_some_and(|value| value.is_ascii_uppercase())
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
        "if" | "for"
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

fn is_ignored_signature_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    matches!(
        trimmed.split_whitespace().next().unwrap_or_default(),
        "if" | "for"
            | "while"
            | "switch"
            | "catch"
            | "return"
            | "throw"
            | "else"
            | "new"
            | "delete"
            | "sizeof"
            | "using"
            | "await"
            | "yield"
    )
}

fn is_ignored_function_name(symbol: &str) -> bool {
    matches!(
        symbol,
        "if" | "for"
            | "while"
            | "switch"
            | "catch"
            | "return"
            | "throw"
            | "else"
            | "new"
            | "delete"
            | "sizeof"
            | "using"
            | "await"
            | "yield"
    )
}

fn is_c_family_access_label(line: &str) -> bool {
    matches!(
        line.trim(),
        "public:"
            | "private:"
            | "protected:"
            | "signals:"
            | "slots:"
            | "public slots:"
            | "private slots:"
            | "protected slots:"
    )
}

fn can_start_c_family_signature(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || is_ignored_signature_line(trimmed) || is_c_family_access_label(trimmed)
    {
        return false;
    }
    if trimmed.contains('(') {
        return true;
    }
    if trimmed.ends_with(';')
        || trimmed.ends_with('{')
        || trimmed.ends_with('}')
        || trimmed.ends_with('=')
        || trimmed.ends_with(',')
    {
        return false;
    }
    let candidate = format!("{trimmed} __codex_monitor_placeholder() {{}}");
    c_family_function_regex().captures(&candidate).is_some()
}

#[derive(Debug, Clone)]
struct CFamilyTypeContext {
    name: String,
    body_depth: usize,
}

#[derive(Debug, Clone, Copy)]
struct CFamilyCallableMatch {
    symbol_start: usize,
    symbol_end: usize,
    constructor_like: bool,
}

fn is_c_family_record_declaration(candidate: &str, symbol_start: usize, language: &str) -> bool {
    matches!(language, "java" | "csharp")
        && candidate[..symbol_start]
            .split_whitespace()
            .any(|token| token == "record")
}

fn is_cpp_family_language(language: &str) -> bool {
    matches!(language, "cpp" | "objective-cpp")
}

fn supports_c_family_type_context(language: &str) -> bool {
    matches!(language, "java" | "csharp") || is_cpp_family_language(language)
}

fn is_c_family_non_callable_declaration(
    candidate: &str,
    symbol_start: usize,
    language: &str,
) -> bool {
    let prefix = candidate[..symbol_start].trim_end();
    let tokens = prefix
        .split(|value: char| !(value.is_ascii_alphanumeric() || value == '_' || value == ':'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    tokens.contains(&"typedef")
        || (language == "csharp" && tokens.contains(&"delegate"))
        || (matches!(language, "csharp") || is_cpp_family_language(language))
            && tokens.last() == Some(&"operator")
}

fn is_c_family_constructor_symbol(symbol: &str, immediate_type_name: Option<&str>) -> bool {
    let parts = symbol.split("::").collect::<Vec<_>>();
    let Some(last) = parts.last() else {
        return false;
    };
    let normalized_last = last.trim_start_matches('~');
    if normalized_last.is_empty() {
        return false;
    }
    if let Some(previous) = parts.iter().rev().nth(1) {
        return previous.trim_start_matches('~') == normalized_last;
    }
    immediate_type_name.is_some_and(|type_name| type_name == normalized_last)
}

fn find_c_family_callable_match(
    candidate: &str,
    immediate_type_name: Option<&str>,
) -> Option<CFamilyCallableMatch> {
    if let Some(captures) = c_family_constructor_regex().captures(candidate) {
        let symbol = captures.get(1)?;
        if is_c_family_constructor_symbol(symbol.as_str(), immediate_type_name) {
            return Some(CFamilyCallableMatch {
                symbol_start: symbol.start(),
                symbol_end: symbol.end(),
                constructor_like: true,
            });
        }
    }
    let captures = c_family_function_regex().captures(candidate)?;
    let symbol = captures.get(1)?;
    Some(CFamilyCallableMatch {
        symbol_start: symbol.start(),
        symbol_end: symbol.end(),
        constructor_like: false,
    })
}

fn immediate_c_family_type_name<'a>(
    contexts: &'a [CFamilyTypeContext],
    brace_depth: usize,
) -> Option<&'a str> {
    contexts
        .last()
        .filter(|context| context.body_depth == brace_depth)
        .map(|context| context.name.as_str())
}

fn prune_c_family_type_contexts(contexts: &mut Vec<CFamilyTypeContext>, brace_depth: usize) {
    while contexts
        .last()
        .is_some_and(|context| brace_depth < context.body_depth)
    {
        contexts.pop();
    }
}

fn count_unquoted_braces(line: &str) -> (usize, usize) {
    let bytes = line.as_bytes();
    let mut open = 0usize;
    let mut close = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for byte in bytes {
        if in_single {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_double = false;
            }
            continue;
        }
        match *byte {
            b'\'' => in_single = true,
            b'"' => in_double = true,
            b'{' => open += 1,
            b'}' => close += 1,
            _ => {}
        }
    }

    (open, close)
}

fn is_probable_c_family_type_declaration_tail(tail: &str) -> bool {
    let trimmed = tail.trim_start();
    trimmed.is_empty()
        || trimmed.starts_with('{')
        || trimmed.starts_with(':')
        || trimmed.starts_with(';')
        || trimmed.starts_with('<')
        || trimmed.starts_with('(')
        || trimmed.starts_with("final")
        || trimmed.starts_with("where")
        || trimmed.starts_with("extends")
        || trimmed.starts_with("implements")
        || trimmed.starts_with("permits")
}

fn find_c_family_type_declaration_name(line: &str, language: &str) -> Option<String> {
    if !supports_c_family_type_context(language) {
        return None;
    }
    c_family_type_declaration_regex()
        .captures(line)
        .and_then(|captures| {
            let tail = captures.get(2)?.as_str();
            is_probable_c_family_type_declaration_tail(tail).then(|| captures.get(1))
        })
        .flatten()
        .map(|value| value.as_str().to_string())
}

fn advance_c_family_type_context(
    line: &str,
    language: &str,
    brace_depth: &mut usize,
    contexts: &mut Vec<CFamilyTypeContext>,
    pending_type_name: &mut Option<String>,
) {
    if !supports_c_family_type_context(language) {
        return;
    }

    prune_c_family_type_contexts(contexts, *brace_depth);

    let trimmed = line.trim();
    let declared_type_name = find_c_family_type_declaration_name(trimmed, language);
    let (open_braces, close_braces) = count_unquoted_braces(line);

    if let Some(type_name) = declared_type_name {
        if open_braces > 0 {
            contexts.push(CFamilyTypeContext {
                name: type_name,
                body_depth: *brace_depth + 1,
            });
            *pending_type_name = None;
        } else if !trimmed.ends_with(';') {
            *pending_type_name = Some(type_name);
        }
    } else if open_braces > 0 {
        if let Some(type_name) = pending_type_name.take() {
            contexts.push(CFamilyTypeContext {
                name: type_name,
                body_depth: *brace_depth + 1,
            });
        }
    }

    *brace_depth += open_braces;
    *brace_depth = brace_depth.saturating_sub(close_braces);
    prune_c_family_type_contexts(contexts, *brace_depth);
}

fn starts_with_ml_function_literal(rhs: &str) -> bool {
    let trimmed = rhs.trim_start();
    matches!(trimmed, "fun" | "function")
        || trimmed.starts_with("fun ")
        || trimmed.starts_with("fun(")
        || trimmed.starts_with("function ")
}

fn is_ml_callable_binding(binding_head: &str, rhs: &str) -> bool {
    let trimmed = binding_head.trim_start();
    (!trimmed.is_empty() && !trimmed.starts_with(':')) || starts_with_ml_function_literal(rhs)
}

fn is_fsharp_callable_member(binding_head: &str) -> bool {
    let trimmed = binding_head.trim_start();
    !trimmed.is_empty() && !trimmed.starts_with(':') && !trimmed.starts_with("with")
}

fn find_matching_paren(value: &str, open_index: usize) -> Option<usize> {
    let bytes = value.as_bytes();
    let mut depth = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(open_index) {
        if in_single {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_double = false;
            }
            continue;
        }
        match *byte {
            b'\'' => in_single = true,
            b'"' => in_double = true,
            b'(' => depth += 1,
            b')' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level<'a>(value: &'a str, delimiter: u8) -> Vec<&'a str> {
    let bytes = value.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut angle_depth = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().enumerate() {
        if in_single {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_double = false;
            }
            continue;
        }
        match *byte {
            b'\'' => in_single = true,
            b'"' => in_double = true,
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b'{' => brace_depth += 1,
            b'}' => brace_depth = brace_depth.saturating_sub(1),
            b'<' => angle_depth += 1,
            b'>' => angle_depth = angle_depth.saturating_sub(1),
            _ if *byte == delimiter
                && paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && angle_depth == 0 =>
            {
                parts.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }

    parts.push(&value[start..]);
    parts
}

fn strip_top_level_suffix<'a>(value: &'a str, delimiter: u8) -> &'a str {
    let parts = split_top_level(value, delimiter);
    if parts.len() > 1 {
        parts[0]
    } else {
        value
    }
}

fn is_cpp_type_like_token(token: &str, return_type_prefix: &str) -> bool {
    matches!(
        token,
        "auto"
            | "bool"
            | "char"
            | "char8_t"
            | "char16_t"
            | "char32_t"
            | "double"
            | "float"
            | "int"
            | "long"
            | "short"
            | "signed"
            | "size_t"
            | "ssize_t"
            | "string"
            | "unsigned"
            | "void"
            | "wchar_t"
    ) || token.ends_with("_t")
        || token
            .chars()
            .next()
            .map(|value| value.is_ascii_uppercase())
            .unwrap_or(false)
        || return_type_prefix
            .split(|value: char| !(value.is_ascii_alphanumeric() || value == '_'))
            .any(|part| part == token)
}

fn looks_like_cpp_parameter_declaration(segment: &str, return_type_prefix: &str) -> bool {
    let segment = strip_top_level_suffix(segment, b'=').trim();
    if segment.is_empty() {
        return false;
    }
    if matches!(segment, "void" | "...") {
        return true;
    }

    let lowered = segment.to_ascii_lowercase();
    if lowered.starts_with("const ")
        || lowered.starts_with("volatile ")
        || lowered.starts_with("mutable ")
        || lowered.starts_with("typename ")
        || lowered.starts_with("class ")
        || lowered.starts_with("struct ")
        || lowered.starts_with("enum ")
        || lowered.starts_with("unsigned ")
        || lowered.starts_with("signed ")
        || lowered.starts_with("short ")
        || lowered.starts_with("long ")
    {
        return true;
    }

    if segment.contains("(*")
        || segment.contains("(&")
        || segment.contains("::")
        || segment.contains('<')
        || segment.contains('>')
        || segment.contains('*')
        || segment.contains('&')
        || segment.contains('[')
        || segment.contains(']')
    {
        return true;
    }

    if segment.contains('"')
        || segment.contains('\'')
        || segment.contains('{')
        || segment.contains('}')
        || segment.contains('.')
        || segment.contains("->")
    {
        return false;
    }

    if (segment.contains('(') || segment.contains(')'))
        && !lowered.starts_with("decltype")
        && !lowered.starts_with("typeof")
    {
        return false;
    }

    let first_non_whitespace = segment.chars().find(|value| !value.is_whitespace());
    if first_non_whitespace
        .map(|value| value.is_ascii_digit())
        .unwrap_or(false)
    {
        return false;
    }

    let identifiers = segment
        .split(|value: char| !(value.is_ascii_alphanumeric() || value == '_'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if identifiers.is_empty() {
        return false;
    }
    if identifiers.len() >= 2 {
        return true;
    }

    is_cpp_type_like_token(identifiers[0], return_type_prefix)
}

fn is_probable_cpp_direct_initialization(candidate: &str, symbol_end: usize) -> bool {
    let trimmed = candidate.trim();
    if !trimmed.ends_with(';') || trimmed.contains('{') {
        return false;
    }

    let after_symbol = &candidate[symbol_end..];
    let Some(open_offset) = after_symbol.find('(') else {
        return false;
    };
    let open_index = symbol_end + open_offset;
    let Some(close_index) = find_matching_paren(candidate, open_index) else {
        return false;
    };

    if candidate[close_index + 1..].trim() != ";" {
        return false;
    }

    let params = &candidate[open_index + 1..close_index];
    let trimmed_params = params.trim();
    if trimmed_params.is_empty() || trimmed_params == "void" {
        return false;
    }

    let return_type_prefix = candidate[..symbol_end].trim();
    let segments = split_top_level(params, b',');
    !segments.is_empty()
        && segments
            .iter()
            .all(|segment| !looks_like_cpp_parameter_declaration(segment, return_type_prefix))
}

fn is_ignored_haskell_name(symbol: &str) -> bool {
    matches!(
        symbol,
        "data" | "type" | "newtype" | "module" | "import" | "class" | "instance" | "where"
    )
}

fn is_ignored_assembly_label(symbol: &str) -> bool {
    symbol.starts_with('.')
        || matches!(
            symbol.to_ascii_lowercase().as_str(),
            "section"
                | "segment"
                | "global"
                | "extern"
                | "public"
                | "private"
                | "text"
                | "data"
                | "bss"
        )
}

fn is_ignored_cobol_name(symbol: &str) -> bool {
    matches!(
        symbol.to_ascii_uppercase().as_str(),
        "IDENTIFICATION"
            | "ENVIRONMENT"
            | "DATA"
            | "PROCEDURE"
            | "WORKING-STORAGE"
            | "LINKAGE"
            | "CONFIGURATION"
            | "INPUT-OUTPUT"
            | "FILE"
            | "SPECIAL-NAMES"
            | "PROGRAM-ID"
            | "FUNCTION-ID"
            | "AUTHOR"
            | "DATE-WRITTEN"
            | "INSTALLATION"
            | "SECURITY"
            | "REMARKS"
            | "END"
    )
}

fn is_ignored_prolog_name(symbol: &str) -> bool {
    matches!(symbol, "true" | "fail")
}

fn normalize_objective_c_selector(signature: &str) -> Option<String> {
    let trimmed = signature.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut selector = String::new();
    for captures in objective_c_selector_part_regex().captures_iter(trimmed) {
        let Some(part) = captures.get(1) else {
            continue;
        };
        selector.push_str(part.as_str());
        selector.push(':');
    }
    if !selector.is_empty() {
        return Some(selector);
    }
    trimmed
        .split_whitespace()
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn looks_like_objective_c(content: &str) -> bool {
    content.contains("@interface")
        || content.contains("@implementation")
        || content.contains("@protocol")
        || content.contains("#import")
        || objective_c_method_regex().is_match(content)
}

fn looks_like_matlab(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("function ") || trimmed.starts_with("classdef ")
    })
}

fn cpp_type_declaration_start_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"\b(?:class|struct)\s+([A-Za-z_][A-Za-z0-9_]*)\b[^{;\n]*\{")
            .expect("valid cpp type declaration start regex")
    })
}

fn find_matching_brace(value: &str, open_index: usize) -> Option<usize> {
    let bytes = value.as_bytes();
    let mut depth = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(open_index) {
        if in_single {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_double = false;
            }
            continue;
        }
        match *byte {
            b'\'' => in_single = true,
            b'"' => in_double = true,
            b'{' => depth += 1,
            b'}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn has_cpp_callable_members(body: &str, type_name: &str) -> bool {
    split_top_level(body, b';').into_iter().any(|segment| {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            return false;
        }
        let candidate = if trimmed.ends_with('}') || trimmed.ends_with('{') {
            trimmed.to_string()
        } else {
            format!("{trimmed};")
        };
        find_c_family_callable_match(&candidate, Some(type_name)).is_some()
    })
}

fn has_cpp_struct_or_class_member_declarations(content: &str) -> bool {
    let mut search_start = 0usize;
    while search_start < content.len() {
        let Some(captures) = cpp_type_declaration_start_regex().captures(&content[search_start..])
        else {
            break;
        };
        let Some(full_match) = captures.get(0) else {
            break;
        };
        let Some(type_name) = captures.get(1).map(|value| value.as_str()) else {
            search_start += full_match.end();
            continue;
        };
        let Some(open_offset) = full_match.as_str().rfind('{') else {
            search_start += full_match.end();
            continue;
        };
        let open_index = search_start + full_match.start() + open_offset;
        let next_search_start = search_start + full_match.end();
        let Some(close_index) = find_matching_brace(content, open_index) else {
            search_start = next_search_start;
            continue;
        };
        if has_cpp_callable_members(&content[open_index + 1..close_index], type_name) {
            return true;
        }
        search_start = close_index + 1;
    }
    false
}

fn looks_like_cpp_header(content: &str) -> bool {
    content.contains("class ")
        || has_cpp_struct_or_class_member_declarations(content)
        || content.contains("namespace ")
        || content.contains("template<")
        || content.contains("template <")
        || content.contains("typename ")
        || content.contains("::")
        || content.contains("using ")
        || content.contains("constexpr")
        || content.contains("noexcept")
        || content.contains("friend ")
        || content.contains("operator ")
        || content.contains("public:")
        || content.contains("private:")
        || content.contains("protected:")
        || content.lines().any(|line| {
            let Some(captures) = c_family_function_regex().captures(line) else {
                return false;
            };
            let Some(symbol) = captures.get(1) else {
                return false;
            };
            is_probable_cpp_direct_initialization(line, symbol.end())
        })
}

fn supported_language(path: &Path) -> Option<(&'static str, Option<&'static str>)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let value = match ext.as_str() {
        "ts" => ("ts", Some("typescript")),
        "tsx" => ("tsx", Some("typescript")),
        "mts" => ("mts", Some("typescript")),
        "cts" => ("cts", Some("typescript")),
        "js" => ("js", Some("javascript")),
        "jsx" => ("jsx", Some("javascript")),
        "mjs" => ("mjs", Some("javascript")),
        "cjs" => ("cjs", Some("javascript")),
        "rs" => ("rs", Some("rust")),
        "py" => ("py", Some("python")),
        "go" => ("go", Some("go")),
        "java" => ("java", Some("java")),
        "c" => ("c", Some("c")),
        "h" => ("h", None),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => ("cpp", Some("cpp")),
        "cs" => ("cs", Some("csharp")),
        "php" | "phtml" => ("php", Some("php")),
        "rb" | "rake" => ("rb", Some("ruby")),
        "swift" => ("swift", Some("swift")),
        "kt" | "kts" => ("kt", Some("kotlin")),
        "scala" | "sc" => ("scala", Some("scala")),
        "dart" => ("dart", Some("dart")),
        "lua" => ("lua", Some("lua")),
        "sh" | "bash" | "zsh" => ("sh", Some("shell")),
        "ps1" | "psm1" | "psd1" => ("ps1", Some("powershell")),
        "pl" | "pm" => ("pl", Some("perl")),
        "r" => ("r", Some("r")),
        "jl" => ("jl", Some("julia")),
        "ex" | "exs" => ("ex", Some("elixir")),
        "erl" => ("erl", Some("erlang")),
        "groovy" | "gvy" | "gradle" => ("groovy", Some("groovy")),
        "clj" | "cljs" | "cljc" => ("clj", Some("clojure")),
        // `.m` is ambiguous: resolve Objective-C vs MATLAB from file contents.
        "m" => ("m", None),
        "mm" => ("mm", Some("objective-cpp")),
        "hs" => ("hs", Some("haskell")),
        "ml" | "mli" => ("ml", Some("ocaml")),
        "fs" | "fsi" | "fsx" => ("fs", Some("fsharp")),
        "vb" => ("vb", Some("vbnet")),
        "pas" | "pp" | "dpr" => ("pas", Some("pascal")),
        "asm" | "s" => ("asm", Some("assembly")),
        "sql" => ("sql", Some("sql")),
        "sas" => ("sas", Some("sas")),
        "f" | "f90" | "f95" | "f03" | "f08" | "for" => ("f", Some("fortran")),
        "cob" | "cbl" | "cpy" => ("cob", Some("cobol")),
        "lisp" | "lsp" | "cl" => ("lisp", Some("common-lisp")),
        "scm" | "ss" => ("scm", Some("scheme")),
        "tcl" => ("tcl", Some("tcl")),
        "pro" | "prolog" => ("pro", Some("prolog")),
        "nim" | "nims" => ("nim", Some("nim")),
        "zig" => ("zig", Some("zig")),
        "sol" => ("sol", Some("solidity")),
        _ => return None,
    };
    Some(value)
}

fn build_multiline_signature_candidate<F>(
    lines: &[&str],
    start: usize,
    should_attempt: F,
    max_lines: usize,
) -> Option<(String, usize)>
where
    F: Fn(&str) -> bool,
{
    let current = lines.get(start)?.trim();
    if current.is_empty() || !should_attempt(current) {
        return None;
    }
    let mut candidate = current.to_string();
    let mut consumed = 0;
    for offset in 1..max_lines {
        let Some(next) = lines.get(start + offset) else {
            break;
        };
        let trimmed = next.trim();
        if trimmed.is_empty() {
            break;
        }
        candidate.push(' ');
        candidate.push_str(trimmed);
        consumed = offset;
    }
    (consumed > 0).then_some((candidate, consumed))
}

fn push_symbol(
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
    path: &str,
    symbol: &str,
    kind: &str,
    language: &str,
) {
    let normalized_path = path.trim();
    let normalized_symbol = symbol.trim();
    if normalized_path.is_empty() || normalized_symbol.is_empty() {
        return;
    }
    let key = (normalized_path.to_string(), normalized_symbol.to_string());
    if !seen.insert(key) {
        return;
    }
    results.push(WorkspaceCallableSymbol {
        path: normalized_path.to_string(),
        symbol: normalized_symbol.to_string(),
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

fn decode_utf16_symbol_file(bytes: &[u8], little_endian: bool) -> Option<String> {
    let chunks = bytes.chunks_exact(2);
    if !chunks.remainder().is_empty() {
        return None;
    }
    let units = chunks
        .map(|chunk| {
            if little_endian {
                u16::from_le_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]])
            }
        })
        .collect::<Vec<_>>();
    String::from_utf16(&units).ok()
}

fn read_symbol_file(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    if let Some(content) = bytes
        .strip_prefix(&[0xFF, 0xFE])
        .and_then(|value| decode_utf16_symbol_file(value, true))
    {
        return Some(content);
    }
    if let Some(content) = bytes
        .strip_prefix(&[0xFE, 0xFF])
        .and_then(|value| decode_utf16_symbol_file(value, false))
    {
        return Some(content);
    }
    if let Some(value) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return std::str::from_utf8(value).ok().map(str::to_owned);
    }
    std::str::from_utf8(&bytes).ok().map(str::to_owned)
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
        let params = captures
            .get(2)
            .map(|value| value.as_str())
            .unwrap_or_default();
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
        let params = captures
            .get(2)
            .map(|value| value.as_str())
            .unwrap_or_default();
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

fn extract_c_family_callables(
    content: &str,
    path: &str,
    extension: &str,
    language: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    let lines = content.lines().collect::<Vec<_>>();
    let mut index = 0;
    let mut brace_depth = 0usize;
    let mut type_contexts = Vec::new();
    let mut pending_type_name = None;
    while let Some(line) = lines.get(index) {
        prune_c_family_type_contexts(&mut type_contexts, brace_depth);
        let immediate_type_name = immediate_c_family_type_name(&type_contexts, brace_depth);
        if is_ignored_signature_line(line) {
            advance_c_family_type_context(
                line,
                language,
                &mut brace_depth,
                &mut type_contexts,
                &mut pending_type_name,
            );
            index += 1;
            continue;
        }
        let mut consumed = 0;
        let matched = if find_c_family_callable_match(line, immediate_type_name).is_some() {
            None
        } else {
            let mut match_result = None;
            let mut last_offset = 0;
            let mut candidate =
                build_multiline_signature_candidate(&lines, index, can_start_c_family_signature, 2);
            while let Some((value, offset)) = candidate {
                if find_c_family_callable_match(&value, immediate_type_name).is_some() {
                    consumed = offset;
                    match_result = Some(value);
                    break;
                }
                if offset <= last_offset {
                    break;
                }
                last_offset = offset;
                candidate = build_multiline_signature_candidate(
                    &lines,
                    index,
                    can_start_c_family_signature,
                    offset + 2,
                );
            }
            let Some(value) = match_result else {
                advance_c_family_type_context(
                    line,
                    language,
                    &mut brace_depth,
                    &mut type_contexts,
                    &mut pending_type_name,
                );
                index += 1;
                continue;
            };
            Some(value)
        };
        let candidate = matched.as_deref().unwrap_or(line);
        let Some(callable_match) = find_c_family_callable_match(candidate, immediate_type_name)
        else {
            advance_c_family_type_context(
                line,
                language,
                &mut brace_depth,
                &mut type_contexts,
                &mut pending_type_name,
            );
            index += 1;
            continue;
        };
        let symbol = &candidate[callable_match.symbol_start..callable_match.symbol_end];
        if is_c_family_record_declaration(candidate, callable_match.symbol_start, language) {
            for consumed_line in &lines[index..=index + consumed] {
                advance_c_family_type_context(
                    consumed_line,
                    language,
                    &mut brace_depth,
                    &mut type_contexts,
                    &mut pending_type_name,
                );
            }
            index += consumed + 1;
            continue;
        }
        if is_c_family_non_callable_declaration(candidate, callable_match.symbol_start, language) {
            for consumed_line in &lines[index..=index + consumed] {
                advance_c_family_type_context(
                    consumed_line,
                    language,
                    &mut brace_depth,
                    &mut type_contexts,
                    &mut pending_type_name,
                );
            }
            index += consumed + 1;
            continue;
        }
        let symbol_end = callable_match.symbol_end;
        if is_ignored_function_name(symbol) {
            for consumed_line in &lines[index..=index + consumed] {
                advance_c_family_type_context(
                    consumed_line,
                    language,
                    &mut brace_depth,
                    &mut type_contexts,
                    &mut pending_type_name,
                );
            }
            index += consumed + 1;
            continue;
        }
        if is_cpp_family_language(language)
            && is_probable_cpp_direct_initialization(candidate, symbol_end)
        {
            for consumed_line in &lines[index..=index + consumed] {
                advance_c_family_type_context(
                    consumed_line,
                    language,
                    &mut brace_depth,
                    &mut type_contexts,
                    &mut pending_type_name,
                );
            }
            index += consumed + 1;
            continue;
        }
        let method_like = is_cpp_family_language(language)
            && (symbol.contains("::") || callable_match.constructor_like);
        push_symbol(
            results,
            seen,
            path,
            symbol,
            normalize_symbol_kind(symbol, extension, method_like),
            language,
        );
        for consumed_line in &lines[index..=index + consumed] {
            advance_c_family_type_context(
                consumed_line,
                language,
                &mut brace_depth,
                &mut type_contexts,
                &mut pending_type_name,
            );
        }
        index += consumed + 1;
    }
}

fn extract_php_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in php_function_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let full_match = captures
            .get(0)
            .map(|value| value.as_str())
            .unwrap_or_default();
        let method_like = full_match.contains("public")
            || full_match.contains("private")
            || full_match.contains("protected")
            || full_match.contains("static");
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, method_like),
            "php",
        );
    }
}

fn extract_ruby_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in ruby_function_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let full_match = captures
            .get(0)
            .map(|value| value.as_str())
            .unwrap_or_default();
        let method_like = full_match.contains("self.") || full_match.contains('.');
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, method_like),
            "ruby",
        );
    }
}

fn extract_swift_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in swift_function_regex().captures_iter(content) {
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                "swift",
            );
        }
    }
}

fn extract_kotlin_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in kotlin_function_regex().captures_iter(content) {
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                "kotlin",
            );
        }
    }
}

fn extract_scala_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    let lines = content.lines().collect::<Vec<_>>();
    let mut index = 0;
    while let Some(line) = lines.get(index) {
        let mut consumed = 0;
        let matched = if scala_function_regex().captures(line).is_some() {
            None
        } else {
            let can_start_scala_signature = |value: &str| {
                let trimmed = value.trim_start();
                trimmed.starts_with("def ")
                    || [
                        "private ",
                        "protected ",
                        "override ",
                        "final ",
                        "abstract ",
                        "sealed ",
                        "implicit ",
                        "lazy ",
                        "case ",
                        "inline ",
                    ]
                    .iter()
                    .any(|modifier| trimmed.starts_with(modifier))
            };
            let mut match_result = None;
            let mut last_offset = 0;
            let mut candidate =
                build_multiline_signature_candidate(&lines, index, can_start_scala_signature, 2);
            while let Some((value, offset)) = candidate {
                if scala_function_regex().captures(&value).is_some() {
                    consumed = offset;
                    match_result = Some(value);
                    break;
                }
                if offset <= last_offset {
                    break;
                }
                last_offset = offset;
                candidate = build_multiline_signature_candidate(
                    &lines,
                    index,
                    can_start_scala_signature,
                    offset + 2,
                );
            }
            let Some(value) = match_result else {
                index += 1;
                continue;
            };
            Some(value)
        };
        let candidate = matched.as_deref().unwrap_or(line);
        let Some(captures) = scala_function_regex().captures(candidate) else {
            index += 1;
            continue;
        };
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                "scala",
            );
        }
        index += consumed + 1;
    }
}

fn extract_dart_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    let lines = content.lines().collect::<Vec<_>>();
    let mut index = 0;
    while let Some(line) = lines.get(index) {
        if is_ignored_signature_line(line) {
            index += 1;
            continue;
        }
        let mut consumed = 0;
        let matched = if dart_function_regex().captures(line).is_some() {
            None
        } else {
            let mut match_result = None;
            let mut last_offset = 0;
            let mut candidate =
                build_multiline_signature_candidate(&lines, index, |value| value.contains('('), 2);
            while let Some((value, offset)) = candidate {
                if dart_function_regex().captures(&value).is_some() {
                    consumed = offset;
                    match_result = Some(value);
                    break;
                }
                if offset <= last_offset {
                    break;
                }
                last_offset = offset;
                candidate = build_multiline_signature_candidate(
                    &lines,
                    index,
                    |line| line.contains('('),
                    offset + 2,
                );
            }
            let Some(value) = match_result else {
                index += 1;
                continue;
            };
            Some(value)
        };
        let candidate = matched.as_deref().unwrap_or(line);
        let Some(captures) = dart_function_regex().captures(candidate) else {
            index += 1;
            continue;
        };
        let Some(symbol) = captures.get(1) else {
            index += consumed + 1;
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "dart",
        );
        index += consumed + 1;
    }
}

fn extract_lua_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in lua_function_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let method_like = captures
            .get(0)
            .map(|value| value.as_str().contains(':'))
            .unwrap_or(false);
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, method_like),
            "lua",
        );
    }
}

fn extract_shell_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in shell_function_regex().captures_iter(content) {
        let symbol = captures
            .get(1)
            .or_else(|| captures.get(2))
            .map(|value| value.as_str());
        let Some(symbol) = symbol else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol,
            normalize_symbol_kind(symbol, extension, false),
            "shell",
        );
    }
}

fn extract_powershell_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in powershell_function_regex().captures_iter(content) {
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                "powershell",
            );
        }
    }
}

fn extract_perl_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in perl_sub_regex().captures_iter(content) {
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                "perl",
            );
        }
    }
}

fn extract_r_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in r_function_regex().captures_iter(content) {
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                "r",
            );
        }
    }
}

fn extract_julia_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in julia_function_regex().captures_iter(content) {
        let symbol = captures
            .get(1)
            .or_else(|| captures.get(2))
            .map(|value| value.as_str());
        let Some(symbol) = symbol else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol,
            normalize_symbol_kind(symbol, extension, false),
            "julia",
        );
    }
}

fn extract_elixir_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in elixir_function_regex().captures_iter(content) {
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                "elixir",
            );
        }
    }
}

fn extract_erlang_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in erlang_function_regex().captures_iter(content) {
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                "erlang",
            );
        }
    }
}

fn extract_groovy_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    if !path.ends_with(".gradle") {
        extract_c_family_callables(content, path, extension, "groovy", results, seen);
    }
    for captures in groovy_def_regex().captures_iter(content) {
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                "groovy",
            );
        }
    }
}

fn extract_clojure_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in clojure_defn_regex().captures_iter(content) {
        if let Some(symbol) = captures.get(1) {
            push_symbol(
                results,
                seen,
                path,
                symbol.as_str(),
                normalize_symbol_kind(symbol.as_str(), extension, false),
                "clojure",
            );
        }
    }
}

fn extract_objective_c_callables(
    content: &str,
    path: &str,
    extension: &str,
    language: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    extract_c_family_callables(content, path, extension, language, results, seen);
    for captures in objective_c_method_regex().captures_iter(content) {
        let Some(signature) = captures.get(1) else {
            continue;
        };
        let Some(selector) = normalize_objective_c_selector(signature.as_str()) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            &selector,
            normalize_symbol_kind(&selector, extension, true),
            language,
        );
    }
}

fn extract_haskell_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in haskell_function_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let symbol = symbol.as_str();
        if is_ignored_haskell_name(symbol) {
            continue;
        }
        push_symbol(
            results,
            seen,
            path,
            symbol,
            normalize_symbol_kind(symbol, extension, false),
            "haskell",
        );
    }
}

fn extract_ocaml_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in ocaml_let_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let binding_head = captures
            .get(2)
            .map(|value| value.as_str())
            .unwrap_or_default();
        let rhs = captures
            .get(3)
            .map(|value| value.as_str())
            .unwrap_or_default();
        if !is_ml_callable_binding(binding_head, rhs) {
            continue;
        }
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "ocaml",
        );
    }
}

fn extract_fsharp_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in fsharp_let_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let binding_head = captures
            .get(2)
            .map(|value| value.as_str())
            .unwrap_or_default();
        let rhs = captures
            .get(3)
            .map(|value| value.as_str())
            .unwrap_or_default();
        if !is_ml_callable_binding(binding_head, rhs) {
            continue;
        }
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "fsharp",
        );
    }
    for captures in fsharp_member_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let binding_head = captures
            .get(2)
            .map(|value| value.as_str())
            .unwrap_or_default();
        if !is_fsharp_callable_member(binding_head) {
            continue;
        }
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, true),
            "fsharp",
        );
    }
}

fn extract_vb_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in vb_callable_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "vbnet",
        );
    }
}

fn extract_pascal_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in pascal_callable_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let symbol = symbol.as_str();
        let method_like = symbol.contains('.');
        push_symbol(
            results,
            seen,
            path,
            symbol,
            normalize_symbol_kind(symbol, extension, method_like),
            "pascal",
        );
    }
}

fn extract_assembly_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in assembly_proc_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let symbol = symbol.as_str();
        if is_ignored_assembly_label(symbol) {
            continue;
        }
        push_symbol(
            results,
            seen,
            path,
            symbol,
            normalize_symbol_kind(symbol, extension, false),
            "assembly",
        );
    }
    for captures in assembly_label_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let symbol = symbol.as_str();
        if is_ignored_assembly_label(symbol) {
            continue;
        }
        push_symbol(
            results,
            seen,
            path,
            symbol,
            normalize_symbol_kind(symbol, extension, false),
            "assembly",
        );
    }
}

fn extract_sql_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in sql_callable_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "sql",
        );
    }
}

fn extract_matlab_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in matlab_function_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "matlab",
        );
    }
}

fn extract_sas_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in sas_macro_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "sas",
        );
    }
}

fn extract_fortran_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in fortran_callable_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "fortran",
        );
    }
}

fn extract_cobol_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in cobol_function_id_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let symbol = symbol.as_str();
        if is_ignored_cobol_name(symbol) {
            continue;
        }
        push_symbol(
            results,
            seen,
            path,
            symbol,
            normalize_symbol_kind(symbol, extension, false),
            "cobol",
        );
    }
    for captures in cobol_entry_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let symbol = symbol.as_str();
        if is_ignored_cobol_name(symbol) {
            continue;
        }
        push_symbol(
            results,
            seen,
            path,
            symbol,
            normalize_symbol_kind(symbol, extension, false),
            "cobol",
        );
    }
    for captures in cobol_paragraph_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let symbol = symbol.as_str();
        if is_ignored_cobol_name(symbol) {
            continue;
        }
        push_symbol(
            results,
            seen,
            path,
            symbol,
            normalize_symbol_kind(symbol, extension, false),
            "cobol",
        );
    }
}

fn extract_common_lisp_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in common_lisp_defun_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "common-lisp",
        );
    }
}

fn extract_scheme_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in scheme_define_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "scheme",
        );
    }
    for captures in scheme_lambda_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "scheme",
        );
    }
}

fn extract_tcl_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in tcl_proc_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "tcl",
        );
    }
}

fn extract_prolog_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in prolog_predicate_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        let symbol = symbol.as_str();
        if is_ignored_prolog_name(symbol) {
            continue;
        }
        push_symbol(
            results,
            seen,
            path,
            symbol,
            normalize_symbol_kind(symbol, extension, false),
            "prolog",
        );
    }
}

fn extract_nim_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in nim_callable_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "nim",
        );
    }
}

fn extract_zig_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in zig_function_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "zig",
        );
    }
}

fn extract_solidity_callables(
    content: &str,
    path: &str,
    extension: &str,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    for captures in solidity_function_regex().captures_iter(content) {
        let Some(symbol) = captures.get(1) else {
            continue;
        };
        push_symbol(
            results,
            seen,
            path,
            symbol.as_str(),
            normalize_symbol_kind(symbol.as_str(), extension, false),
            "solidity",
        );
    }
}

fn extract_callables_for_file(
    content: &str,
    path: &str,
    extension: &str,
    language: Option<&str>,
    results: &mut Vec<WorkspaceCallableSymbol>,
    seen: &mut HashSet<(String, String)>,
) {
    if extension == "m" {
        if looks_like_objective_c(content) {
            extract_objective_c_callables(content, path, extension, "objective-c", results, seen);
            return;
        }
        if looks_like_matlab(content) {
            extract_matlab_callables(content, path, extension, results, seen);
            return;
        }
        // Plain C helpers are valid in Objective-C implementation files, so keep
        // the Objective-C extractor as the safe fallback for ambiguous `.m` files.
        extract_objective_c_callables(content, path, extension, "objective-c", results, seen);
        return;
    }
    if extension == "h" {
        if looks_like_objective_c(content) {
            extract_objective_c_callables(content, path, extension, "objective-c", results, seen);
            return;
        }
        let language = if looks_like_cpp_header(content) {
            "cpp"
        } else {
            "c"
        };
        extract_c_family_callables(content, path, extension, language, results, seen);
        return;
    }
    let Some(language) = language else {
        return;
    };
    match language {
        "typescript" | "javascript" => {
            extract_ts_callables(content, path, extension, language, results, seen)
        }
        "rust" => extract_rust_callables(content, path, extension, results, seen),
        "python" => extract_python_callables(content, path, extension, results, seen),
        "go" => extract_go_callables(content, path, extension, results, seen),
        "java" | "c" | "cpp" | "csharp" => {
            extract_c_family_callables(content, path, extension, language, results, seen)
        }
        "php" => extract_php_callables(content, path, extension, results, seen),
        "ruby" => extract_ruby_callables(content, path, extension, results, seen),
        "swift" => extract_swift_callables(content, path, extension, results, seen),
        "kotlin" => extract_kotlin_callables(content, path, extension, results, seen),
        "scala" => extract_scala_callables(content, path, extension, results, seen),
        "dart" => extract_dart_callables(content, path, extension, results, seen),
        "lua" => extract_lua_callables(content, path, extension, results, seen),
        "shell" => extract_shell_callables(content, path, extension, results, seen),
        "powershell" => extract_powershell_callables(content, path, extension, results, seen),
        "perl" => extract_perl_callables(content, path, extension, results, seen),
        "r" => extract_r_callables(content, path, extension, results, seen),
        "julia" => extract_julia_callables(content, path, extension, results, seen),
        "elixir" => extract_elixir_callables(content, path, extension, results, seen),
        "erlang" => extract_erlang_callables(content, path, extension, results, seen),
        "groovy" => extract_groovy_callables(content, path, extension, results, seen),
        "clojure" => extract_clojure_callables(content, path, extension, results, seen),
        "objective-c" | "objective-cpp" => {
            extract_objective_c_callables(content, path, extension, language, results, seen)
        }
        "haskell" => extract_haskell_callables(content, path, extension, results, seen),
        "ocaml" => extract_ocaml_callables(content, path, extension, results, seen),
        "fsharp" => extract_fsharp_callables(content, path, extension, results, seen),
        "vbnet" => extract_vb_callables(content, path, extension, results, seen),
        "pascal" => extract_pascal_callables(content, path, extension, results, seen),
        "assembly" => extract_assembly_callables(content, path, extension, results, seen),
        "sql" => extract_sql_callables(content, path, extension, results, seen),
        "sas" => extract_sas_callables(content, path, extension, results, seen),
        "fortran" => extract_fortran_callables(content, path, extension, results, seen),
        "cobol" => extract_cobol_callables(content, path, extension, results, seen),
        "common-lisp" => extract_common_lisp_callables(content, path, extension, results, seen),
        "scheme" => extract_scheme_callables(content, path, extension, results, seen),
        "tcl" => extract_tcl_callables(content, path, extension, results, seen),
        "prolog" => extract_prolog_callables(content, path, extension, results, seen),
        "nim" => extract_nim_callables(content, path, extension, results, seen),
        "zig" => extract_zig_callables(content, path, extension, results, seen),
        "solidity" => extract_solidity_callables(content, path, extension, results, seen),
        _ => {}
    }
}

fn list_workspace_callable_symbols(
    root: &PathBuf,
    max_symbols: usize,
) -> Vec<WorkspaceCallableSymbol> {
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
        if should_skip_generated_symbol_file(entry.path()) {
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
        let Some(content) = read_symbol_file(entry.path()) else {
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
    Ok(list_workspace_callable_symbols(
        &root,
        MAX_WORKSPACE_SYMBOLS,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{WorkspaceKind, WorkspaceSettings};
    use std::time::{SystemTime, UNIX_EPOCH};

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

    fn write_file_bytes(root: &Path, path: &str, content: &[u8]) {
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(target, content).expect("write file");
    }

    #[derive(Clone, Copy)]
    struct LanguageCoverageCase {
        path: &'static str,
        content: &'static str,
        symbol: &'static str,
        kind: &'static str,
        extracted_language: &'static str,
        declared_language: Option<&'static str>,
    }

    fn language_coverage_cases() -> &'static [LanguageCoverageCase] {
        &[
            LanguageCoverageCase {
                path: "src/render_page.ts",
                content: "export function renderPage(): void {}\n",
                symbol: "renderPage",
                kind: "function",
                extracted_language: "typescript",
                declared_language: Some("typescript"),
            },
            LanguageCoverageCase {
                path: "src/render_page.js",
                content: "export function renderPage() {}\n",
                symbol: "renderPage",
                kind: "function",
                extracted_language: "javascript",
                declared_language: Some("javascript"),
            },
            LanguageCoverageCase {
                path: "src/lib.rs",
                content: "pub fn build() {}\n",
                symbol: "build",
                kind: "function",
                extracted_language: "rust",
                declared_language: Some("rust"),
            },
            LanguageCoverageCase {
                path: "scripts/main.py",
                content: "def helper():\n    pass\n",
                symbol: "helper",
                kind: "function",
                extracted_language: "python",
                declared_language: Some("python"),
            },
            LanguageCoverageCase {
                path: "pkg/service.go",
                content: "func Build() {}\n",
                symbol: "Build",
                kind: "function",
                extracted_language: "go",
                declared_language: Some("go"),
            },
            LanguageCoverageCase {
                path: "src/App.java",
                content: "public class App {\n  public static void runApp() {}\n}\n",
                symbol: "runApp",
                kind: "function",
                extracted_language: "java",
                declared_language: Some("java"),
            },
            LanguageCoverageCase {
                path: "src/app.c",
                content: "static int c_run(void) { return 0; }\n",
                symbol: "c_run",
                kind: "function",
                extracted_language: "c",
                declared_language: Some("c"),
            },
            LanguageCoverageCase {
                path: "src/app.cpp",
                content: "int Widget::paint() { return 0; }\n",
                symbol: "Widget::paint",
                kind: "method",
                extracted_language: "cpp",
                declared_language: Some("cpp"),
            },
            LanguageCoverageCase {
                path: "src/App.cs",
                content: "public class App {\n  public async Task RunAsync() { }\n}\n",
                symbol: "RunAsync",
                kind: "function",
                extracted_language: "csharp",
                declared_language: Some("csharp"),
            },
            LanguageCoverageCase {
                path: "src/index.php",
                content: "<?php\nclass App {\n  public function renderPage() {}\n}\n",
                symbol: "renderPage",
                kind: "method",
                extracted_language: "php",
                declared_language: Some("php"),
            },
            LanguageCoverageCase {
                path: "src/app.rb",
                content: "class App\n  def self.bootstrap!\n  end\nend\n",
                symbol: "bootstrap!",
                kind: "method",
                extracted_language: "ruby",
                declared_language: Some("ruby"),
            },
            LanguageCoverageCase {
                path: "src/App.swift",
                content: "final class App {\n  func loadHome() {}\n}\n",
                symbol: "loadHome",
                kind: "function",
                extracted_language: "swift",
                declared_language: Some("swift"),
            },
            LanguageCoverageCase {
                path: "src/App.kt",
                content: "class App {\n  fun refreshState() {}\n}\n",
                symbol: "refreshState",
                kind: "function",
                extracted_language: "kotlin",
                declared_language: Some("kotlin"),
            },
            LanguageCoverageCase {
                path: "src/App.scala",
                content: "object App {\n  def computeState(): Int = 1\n}\n",
                symbol: "computeState",
                kind: "function",
                extracted_language: "scala",
                declared_language: Some("scala"),
            },
            LanguageCoverageCase {
                path: "src/app.dart",
                content: "String buildLabel() => \"ok\";\n",
                symbol: "buildLabel",
                kind: "function",
                extracted_language: "dart",
                declared_language: Some("dart"),
            },
            LanguageCoverageCase {
                path: "src/app.lua",
                content: "local function run_lua() end\n",
                symbol: "run_lua",
                kind: "function",
                extracted_language: "lua",
                declared_language: Some("lua"),
            },
            LanguageCoverageCase {
                path: "scripts/app.sh",
                content: "run_shell() { :; }\n",
                symbol: "run_shell",
                kind: "function",
                extracted_language: "shell",
                declared_language: Some("shell"),
            },
            LanguageCoverageCase {
                path: "scripts/App.ps1",
                content: "function Invoke-Thing { }\n",
                symbol: "Invoke-Thing",
                kind: "function",
                extracted_language: "powershell",
                declared_language: Some("powershell"),
            },
            LanguageCoverageCase {
                path: "scripts/app.pl",
                content: "sub run_perl { return 1; }\n",
                symbol: "run_perl",
                kind: "function",
                extracted_language: "perl",
                declared_language: Some("perl"),
            },
            LanguageCoverageCase {
                path: "scripts/app.R",
                content: "build_plot <- function(x) { x }\n",
                symbol: "build_plot",
                kind: "function",
                extracted_language: "r",
                declared_language: Some("r"),
            },
            LanguageCoverageCase {
                path: "src/app.jl",
                content: "function run_julia(x)\n  x\nend\n",
                symbol: "run_julia",
                kind: "function",
                extracted_language: "julia",
                declared_language: Some("julia"),
            },
            LanguageCoverageCase {
                path: "lib/app.ex",
                content: "defmodule App do\n  def render_view(assigns), do: assigns\nend\n",
                symbol: "render_view",
                kind: "function",
                extracted_language: "elixir",
                declared_language: Some("elixir"),
            },
            LanguageCoverageCase {
                path: "src/app.erl",
                content: "start_server() -> ok.\n",
                symbol: "start_server",
                kind: "function",
                extracted_language: "erlang",
                declared_language: Some("erlang"),
            },
            LanguageCoverageCase {
                path: "build.gradle",
                content: "def configureBuild() { }\n",
                symbol: "configureBuild",
                kind: "function",
                extracted_language: "groovy",
                declared_language: Some("groovy"),
            },
            LanguageCoverageCase {
                path: "src/app.clj",
                content: "(defn render-page [x] x)\n",
                symbol: "render-page",
                kind: "function",
                extracted_language: "clojure",
                declared_language: Some("clojure"),
            },
            LanguageCoverageCase {
                path: "ios/AppDelegate.m",
                content: "- (void)loadHome { }\n",
                symbol: "loadHome",
                kind: "method",
                extracted_language: "objective-c",
                declared_language: None,
            },
            LanguageCoverageCase {
                path: "ios/Bridge.mm",
                content: "- (void)bridgeCall { }\n",
                symbol: "bridgeCall",
                kind: "method",
                extracted_language: "objective-cpp",
                declared_language: Some("objective-cpp"),
            },
            LanguageCoverageCase {
                path: "src/App.hs",
                content: "renderPage value = value\n",
                symbol: "renderPage",
                kind: "function",
                extracted_language: "haskell",
                declared_language: Some("haskell"),
            },
            LanguageCoverageCase {
                path: "src/app.ml",
                content: "let render_page value = value\n",
                symbol: "render_page",
                kind: "function",
                extracted_language: "ocaml",
                declared_language: Some("ocaml"),
            },
            LanguageCoverageCase {
                path: "src/App.fs",
                content: "let renderPage value = value\n",
                symbol: "renderPage",
                kind: "function",
                extracted_language: "fsharp",
                declared_language: Some("fsharp"),
            },
            LanguageCoverageCase {
                path: "src/App.vb",
                content: "Public Function RenderPage() As Integer\nEnd Function\n",
                symbol: "RenderPage",
                kind: "function",
                extracted_language: "vbnet",
                declared_language: Some("vbnet"),
            },
            LanguageCoverageCase {
                path: "src/app.pas",
                content: "procedure RenderPage;\nbegin\nend;\n",
                symbol: "RenderPage",
                kind: "function",
                extracted_language: "pascal",
                declared_language: Some("pascal"),
            },
            LanguageCoverageCase {
                path: "src/app.asm",
                content: "render_page PROC\nrender_page ENDP\n",
                symbol: "render_page",
                kind: "function",
                extracted_language: "assembly",
                declared_language: Some("assembly"),
            },
            LanguageCoverageCase {
                path: "db/functions.sql",
                content: "CREATE FUNCTION render_page() RETURNS void AS $$ BEGIN END; $$ LANGUAGE plpgsql;\n",
                symbol: "render_page",
                kind: "function",
                extracted_language: "sql",
                declared_language: Some("sql"),
            },
            LanguageCoverageCase {
                path: "matlab/render_page.m",
                content: "function y = render_page(x)\ny = x;\nend\n",
                symbol: "render_page",
                kind: "function",
                extracted_language: "matlab",
                declared_language: None,
            },
            LanguageCoverageCase {
                path: "scripts/app.sas",
                content: "%macro render_page();\n%mend;\n",
                symbol: "render_page",
                kind: "function",
                extracted_language: "sas",
                declared_language: Some("sas"),
            },
            LanguageCoverageCase {
                path: "src/app.f90",
                content: "subroutine render_page()\nend subroutine render_page\n",
                symbol: "render_page",
                kind: "function",
                extracted_language: "fortran",
                declared_language: Some("fortran"),
            },
            LanguageCoverageCase {
                path: "src/app.cob",
                content: "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. APP.\n       PROCEDURE DIVISION.\n       RENDER-PAGE.\n           GOBACK.\n",
                symbol: "RENDER-PAGE",
                kind: "function",
                extracted_language: "cobol",
                declared_language: Some("cobol"),
            },
            LanguageCoverageCase {
                path: "src/app.lisp",
                content: "(defun render-page (x) x)\n",
                symbol: "render-page",
                kind: "function",
                extracted_language: "common-lisp",
                declared_language: Some("common-lisp"),
            },
            LanguageCoverageCase {
                path: "src/app.scm",
                content: "(define (render-page x) x)\n",
                symbol: "render-page",
                kind: "function",
                extracted_language: "scheme",
                declared_language: Some("scheme"),
            },
            LanguageCoverageCase {
                path: "scripts/app.tcl",
                content: "proc render_page {value} { return $value }\n",
                symbol: "render_page",
                kind: "function",
                extracted_language: "tcl",
                declared_language: Some("tcl"),
            },
            LanguageCoverageCase {
                path: "logic/app.pro",
                content: "render_page(X) :- true.\n",
                symbol: "render_page",
                kind: "function",
                extracted_language: "prolog",
                declared_language: Some("prolog"),
            },
            LanguageCoverageCase {
                path: "src/app.nim",
                content: "proc renderPage(x: int): int =\n  x\n",
                symbol: "renderPage",
                kind: "function",
                extracted_language: "nim",
                declared_language: Some("nim"),
            },
            LanguageCoverageCase {
                path: "src/app.zig",
                content: "pub fn renderPage() void {}\n",
                symbol: "renderPage",
                kind: "function",
                extracted_language: "zig",
                declared_language: Some("zig"),
            },
            LanguageCoverageCase {
                path: "src/App.sol",
                content: "contract App {\n  function renderPage() public {}\n}\n",
                symbol: "renderPage",
                kind: "function",
                extracted_language: "solidity",
                declared_language: Some("solidity"),
            },
        ]
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
    fn extracts_callable_symbols_for_each_supported_language() {
        let root = temp_workspace_root("language-coverage");
        let cases = language_coverage_cases();
        let languages = cases
            .iter()
            .map(|case| case.extracted_language)
            .collect::<HashSet<_>>();
        assert_eq!(
            languages.len(),
            cases.len(),
            "expected one coverage case per extracted language"
        );

        for case in cases {
            write_file(&root, case.path, case.content);
            let declared_language = supported_language(Path::new(case.path))
                .and_then(|(_, language)| language);
            assert_eq!(
                declared_language,
                case.declared_language,
                "unexpected declared language for {}",
                case.path
            );
        }

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<HashSet<_>>();

        for case in cases {
            let key = format!(
                "{}:{}:{}:{}",
                case.path, case.symbol, case.kind, case.extracted_language
            );
            assert!(
                keys.contains(&key),
                "missing callable coverage key `{key}` for {}",
                case.path
            );
        }

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
        write_file(
            &root,
            "node_modules/pkg/index.ts",
            "export function ignored() {}",
        );
        write_file(&root, "dist/build.ts", "export function ignoredBuild() {}");
        write_file(
            &root,
            "build/generated.java",
            "public class Gen { void ignored() {} }",
        );
        write_file(&root, "Pods/App.m", "- (void)ignoredPods { }\n");
        write_file(
            &root,
            "vendor/App.php",
            "<?php function ignoredVendor() {}\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| format!("{}:{}", entry.path, entry.symbol))
            .collect::<Vec<_>>();

        assert_eq!(keys, vec!["src/feature.ts:duplicate".to_string()]);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skips_generated_symbol_files_by_suffix() {
        let root = temp_workspace_root("generated-files");
        write_file(
            &root,
            "src/feature.ts",
            "export function featureEntry() {}\n",
        );
        write_file(
            &root,
            "src/models.g.dart",
            "String generatedModel() => \"ok\";\n",
        );
        write_file(
            &root,
            "src/messages.pb.go",
            "package main\nfunc GeneratedProto() {}\n",
        );
        write_file(
            &root,
            "src/messages_pb2.py",
            "def generated_proto():\n    pass\n",
        );
        write_file(
            &root,
            "src/Form.Designer.cs",
            "public class FormDesigner {\n  public void InitializeComponent() { }\n}\n",
        );
        write_file(
            &root,
            "src/Form.generated.cs",
            "public class GeneratedForm {\n  public void BuildGenerated() { }\n}\n",
        );
        write_file(
            &root,
            "src/Generated.g.cs",
            "public class GeneratedPartial {\n  public void BuildPartial() { }\n}\n",
        );
        write_file(
            &root,
            "src/Generated.g.vb",
            "Public Class GeneratedView\n  Public Sub BuildGenerated()\n  End Sub\nEnd Class\n",
        );
        write_file(
            &root,
            "obj/Debug/net8.0/AssemblyInfo.cs",
            "public class GeneratedAssemblyInfo {\n  public void BuildAssemblyInfo() { }\n}\n",
        );
        write_file(
            &root,
            "Obj/Debug/net8.0/GeneratorOutput.vb",
            "Public Class GeneratedOutput\n  Public Sub BuildOutput()\n  End Sub\nEnd Class\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| format!("{}:{}", entry.path, entry.symbol))
            .collect::<Vec<_>>();

        assert_eq!(keys, vec!["src/feature.ts:featureEntry".to_string()]);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recognizes_symbol_skip_directories() {
        for name in [
            ".git",
            "node_modules",
            "dist",
            "target",
            "release-artifacts",
            "build",
            "Pods",
            "vendor",
            ".gradle",
            ".dart_tool",
            ".build",
            "Carthage",
            "DerivedData",
            "_build",
            "dist-newstyle",
            ".stack-work",
            ".venv",
            "venv",
            "site-packages",
            "__pycache__",
            ".tox",
            ".nox",
            ".mypy_cache",
            ".pytest_cache",
            "bazel-bin",
            "bazel-out",
            "bazel-testlogs",
            "buck-out",
            "cmake-build-debug",
            "cmake-build-release",
        ] {
            assert!(should_skip_dir(name), "expected {name} to be skipped");
        }

        for name in ["src", "bin", "generated", "cmake", "cmake_build_debug"] {
            assert!(
                !should_skip_dir(name),
                "expected {name} to remain indexable"
            );
        }
    }

    #[test]
    fn extracts_additional_supported_language_callables() {
        let root = temp_workspace_root("additional");
        write_file(
            &root,
            "src/App.java",
            "public class App {\n  public static void runApp() {}\n}\n",
        );
        write_file(&root, "src/app.c", "static int c_run(void) { return 0; }");
        write_file(&root, "src/app.cpp", "int Widget::paint() { return 0; }");
        write_file(
            &root,
            "src/App.cs",
            "public class App {\n  public async Task RunAsync() { }\n}\n",
        );
        write_file(
            &root,
            "src/index.php",
            "<?php\nclass App {\n  public function renderPage() {}\n}\n",
        );
        write_file(
            &root,
            "src/app.rb",
            "class App\n  def self.bootstrap!\n  end\nend",
        );
        write_file(
            &root,
            "src/App.swift",
            "final class App {\n  func loadHome() {}\n}\n",
        );
        write_file(
            &root,
            "src/App.kt",
            "class App {\n  suspend fun refreshState() {}\n}\nfun String.slugify() = lowercase()\n",
        );
        write_file(
            &root,
            "src/App.scala",
            "object App {\n  def computeState(): Int = 1\n}\n",
        );
        write_file(&root, "src/app.dart", "String buildLabel() => \"ok\";");
        write_file(&root, "src/app.lua", "local function run_lua() end");
        write_file(&root, "scripts/app.sh", "run_shell() { :; }");
        write_file(&root, "scripts/App.ps1", "function Invoke-Thing { }");
        write_file(&root, "scripts/app.pl", "sub run_perl { return 1; }");
        write_file(&root, "scripts/app.R", "build_plot <- function(x) { x }");
        write_file(&root, "src/app.jl", "function run_julia(x)\n  x\nend");
        write_file(
            &root,
            "lib/app.ex",
            "defmodule App do\n  def render_view(assigns), do: assigns\nend",
        );
        write_file(&root, "src/app.erl", "start_server() -> ok.");
        write_file(&root, "build.gradle", "def configureBuild() { }");
        write_file(&root, "src/app.clj", "(defn render-page [x] x)");

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"src/App.java:runApp:function:java".to_string()));
        assert!(keys.contains(&"src/app.c:c_run:function:c".to_string()));
        assert!(keys.contains(&"src/app.cpp:Widget::paint:method:cpp".to_string()));
        assert!(keys.contains(&"src/App.cs:RunAsync:function:csharp".to_string()));
        assert!(keys.contains(&"src/index.php:renderPage:method:php".to_string()));
        assert!(keys.contains(&"src/app.rb:bootstrap!:method:ruby".to_string()));
        assert!(keys.contains(&"src/App.swift:loadHome:function:swift".to_string()));
        assert!(keys.contains(&"src/App.kt:refreshState:function:kotlin".to_string()));
        assert!(keys.contains(&"src/App.kt:slugify:function:kotlin".to_string()));
        assert!(keys.contains(&"src/App.scala:computeState:function:scala".to_string()));
        assert!(keys.contains(&"src/app.dart:buildLabel:function:dart".to_string()));
        assert!(keys.contains(&"src/app.lua:run_lua:function:lua".to_string()));
        assert!(keys.contains(&"scripts/app.sh:run_shell:function:shell".to_string()));
        assert!(keys.contains(&"scripts/App.ps1:Invoke-Thing:function:powershell".to_string()));
        assert!(keys.contains(&"scripts/app.pl:run_perl:function:perl".to_string()));
        assert!(keys.contains(&"scripts/app.R:build_plot:function:r".to_string()));
        assert!(keys.contains(&"src/app.jl:run_julia:function:julia".to_string()));
        assert!(keys.contains(&"lib/app.ex:render_view:function:elixir".to_string()));
        assert!(keys.contains(&"src/app.erl:start_server:function:erlang".to_string()));
        assert!(keys.contains(&"build.gradle:configureBuild:function:groovy".to_string()));
        assert!(keys.contains(&"src/app.clj:render-page:function:clojure".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_even_more_supported_language_callables() {
        let root = temp_workspace_root("additional-more");
        write_file(
            &root,
            "ios/AppDelegate.m",
            "- (void)loadHome:(NSString *)title options:(id)options { }\n",
        );
        write_file(&root, "ios/Bridge.mm", "- (void)bridgeCall { }\n");
        write_file(&root, "src/App.hs", "renderPage value = value\n");
        write_file(&root, "src/app.ml", "let rec render_page value = value\n");
        write_file(
            &root,
            "src/App.fs",
            "type App() =\n    member this.Refresh value = value\nlet renderPage value = value\n",
        );
        write_file(
            &root,
            "src/App.vb",
            "Public Function RenderPage() As Integer\nEnd Function\n",
        );
        write_file(&root, "src/app.pas", "procedure RenderPage;\nbegin\nend;\n");
        write_file(&root, "src/app.asm", "render_page PROC\nrender_page ENDP\n");
        write_file(
            &root,
            "db/functions.sql",
            "CREATE FUNCTION render_page() RETURNS void AS $$ BEGIN END; $$ LANGUAGE plpgsql;\n",
        );
        write_file(
            &root,
            "matlab/render_page.m",
            "function y = render_page(x)\ny = x;\nend\n",
        );
        write_file(&root, "scripts/app.sas", "%macro render_page();\n%mend;\n");
        write_file(
            &root,
            "src/app.f90",
            "subroutine render_page()\nend subroutine render_page\n",
        );
        write_file(
            &root,
            "src/app.cob",
            "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. APP.\n       PROCEDURE DIVISION.\n       RENDER-PAGE.\n           GOBACK.\n",
        );
        write_file(&root, "src/app.lisp", "(defun render-page (x) x)\n");
        write_file(&root, "src/app.scm", "(define (render-page x) x)\n");
        write_file(
            &root,
            "scripts/app.tcl",
            "proc render_page {value} { return $value }\n",
        );
        write_file(&root, "logic/app.pro", "render_page(X) :- true.\n");
        write_file(
            &root,
            "src/app.nim",
            "proc renderPage(x: int): int =\n  x\n",
        );
        write_file(&root, "src/app.zig", "pub fn renderPage() void {}\n");
        write_file(
            &root,
            "src/App.sol",
            "contract App {\n  function renderPage() public {}\n}\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(
            keys.contains(&"ios/AppDelegate.m:loadHome:options::method:objective-c".to_string())
        );
        assert!(keys.contains(&"ios/Bridge.mm:bridgeCall:method:objective-cpp".to_string()));
        assert!(keys.contains(&"src/App.hs:renderPage:function:haskell".to_string()));
        assert!(keys.contains(&"src/app.ml:render_page:function:ocaml".to_string()));
        assert!(keys.contains(&"src/App.fs:renderPage:function:fsharp".to_string()));
        assert!(keys.contains(&"src/App.fs:Refresh:method:fsharp".to_string()));
        assert!(keys.contains(&"src/App.vb:RenderPage:function:vbnet".to_string()));
        assert!(keys.contains(&"src/app.pas:RenderPage:function:pascal".to_string()));
        assert!(keys.contains(&"src/app.asm:render_page:function:assembly".to_string()));
        assert!(keys.contains(&"db/functions.sql:render_page:function:sql".to_string()));
        assert!(keys.contains(&"matlab/render_page.m:render_page:function:matlab".to_string()));
        assert!(keys.contains(&"scripts/app.sas:render_page:function:sas".to_string()));
        assert!(keys.contains(&"src/app.f90:render_page:function:fortran".to_string()));
        assert!(keys.contains(&"src/app.cob:RENDER-PAGE:function:cobol".to_string()));
        assert!(keys.contains(&"src/app.lisp:render-page:function:common-lisp".to_string()));
        assert!(keys.contains(&"src/app.scm:render-page:function:scheme".to_string()));
        assert!(keys.contains(&"scripts/app.tcl:render_page:function:tcl".to_string()));
        assert!(keys.contains(&"logic/app.pro:render_page:function:prolog".to_string()));
        assert!(keys.contains(&"src/app.nim:renderPage:function:nim".to_string()));
        assert!(keys.contains(&"src/app.zig:renderPage:function:zig".to_string()));
        assert!(keys.contains(&"src/App.sol:renderPage:function:solidity".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn treats_m_sources_as_ambiguous_objective_c_or_matlab_support() {
        assert_eq!(
            supported_language(Path::new("ios/AppDelegate.m")),
            Some(("m", None))
        );
        assert_eq!(
            supported_language(Path::new("matlab/render_page.m")),
            Some(("m", None))
        );
        assert_eq!(
            supported_language(Path::new("ios/AppDelegate.h")),
            Some(("h", None))
        );
    }

    #[test]
    fn extracts_wrapped_signatures_and_plain_c_m_sources() {
        let root = temp_workspace_root("wrapped-signatures");
        write_file(
            &root,
            "src/App.java",
            "public class App {\n  public static void runApp(\n    String input\n  ) {}\n}\n",
        );
        write_file(
            &root,
            "src/SplitReturn.java",
            "public class SplitReturn {\n  public Task\n  runAsync() { return null; }\n}\n",
        );
        write_file(
            &root,
            "src/split_return.c",
            "static int\nhelper_count(void) { return 1; }\n",
        );
        write_file(
            &root,
            "src/app.dart",
            "String buildLabel(\n  String input,\n) => input;\n",
        );
        write_file(
            &root,
            "src/App.scala",
            "object App {\n  def computeState(\n    input: Int\n  ): Int = input\n}\n",
        );
        write_file(
            &root,
            "src/Modifiers.scala",
            "object App {\n  private def computeState(\n    input: Int\n  ): Int = input\n  override def renderState(\n    input: Int\n  ): Int = input\n}\n",
        );
        write_file(
            &root,
            "ios/Helpers.m",
            "static int helper_count(void) { return 1; }\n",
        );
        write_file(
            &root,
            "ios/AppDelegate.h",
            "@interface AppDelegate : NSObject\n- (void)loadHome:(NSString *)title options:(id)options;\n+ (instancetype)sharedDelegate;\n@end\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"src/App.java:runApp:function:java".to_string()));
        assert!(keys.contains(&"src/SplitReturn.java:runAsync:function:java".to_string()));
        assert!(keys.contains(&"src/app.dart:buildLabel:function:dart".to_string()));
        assert!(keys.contains(&"src/App.scala:computeState:function:scala".to_string()));
        assert!(keys.contains(&"src/Modifiers.scala:computeState:function:scala".to_string()));
        assert!(keys.contains(&"src/Modifiers.scala:renderState:function:scala".to_string()));
        assert!(keys.contains(&"src/split_return.c:helper_count:function:c".to_string()));
        assert!(
            keys.contains(&"ios/AppDelegate.h:loadHome:options::method:objective-c".to_string())
        );
        assert!(keys.contains(&"ios/AppDelegate.h:sharedDelegate:method:objective-c".to_string()));
        assert!(keys.contains(&"ios/Helpers.m:helper_count:function:objective-c".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_dart_named_and_initializer_list_constructors() {
        let root = temp_workspace_root("dart-constructors");
        write_file(
            &root,
            "src/user.dart",
            "class User {\n  User(\n    this.id,\n  ) : createdAt = DateTime.now() {}\n\n  factory User.fromJson(\n    Map<String, dynamic> json,\n  ) => User(json['id'] as String);\n\n  final String id;\n  final DateTime createdAt;\n}\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"src/user.dart:User:function:dart".to_string()));
        assert!(keys.contains(&"src/user.dart:User.fromJson:function:dart".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_wrapped_objective_c_selectors() {
        let root = temp_workspace_root("objective-c-wrapped-selectors");
        write_file(
            &root,
            "ios/AppDelegate.h",
            "@interface AppDelegate : NSObject\n- (void)loadHome:(NSString *)title\n         options:(NSDictionary *)options;\n- (instancetype)initWithName:(NSString *)name\n                     options:(NSDictionary *)options {\n  return self;\n}\n@end\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(
            keys.contains(&"ios/AppDelegate.h:loadHome:options::method:objective-c".to_string())
        );
        assert!(keys
            .contains(&"ios/AppDelegate.h:initWithName:options::method:objective-c".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_java_and_cpp_constructors_without_return_types() {
        let root = temp_workspace_root("c-family-constructors");
        write_file(&root, "src/User.java", "class User {\n  User() {}\n}\n");
        write_file(
            &root,
            "src/Widget.hpp",
            "class Widget {\npublic:\n  Widget();\n};\n",
        );
        write_file(&root, "src/Widget.cpp", "Widget::Widget() {}\n");

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"src/User.java:User:function:java".to_string()));
        assert!(keys.contains(&"src/Widget.hpp:Widget:method:cpp".to_string()));
        assert!(keys.contains(&"src/Widget.cpp:Widget::Widget:method:cpp".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classifies_struct_only_cpp_headers_without_misclassifying_plain_c_structs() {
        let root = temp_workspace_root("struct-only-cpp-headers");
        write_file(
            &root,
            "include/Widget.h",
            "struct Widget {\n  Widget();\n  void run();\n};\n",
        );
        write_file(
            &root,
            "include/plain.h",
            "struct Plain {\n  int value;\n};\nint render_plain(void);\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"include/Widget.h:Widget:method:cpp".to_string()));
        assert!(keys.contains(&"include/Widget.h:run:function:cpp".to_string()));
        assert!(keys.contains(&"include/plain.h:render_plain:function:c".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classifies_struct_only_cpp_headers_with_nested_braces() {
        let root = temp_workspace_root("struct-only-cpp-headers-nested");
        write_file(
            &root,
            "include/Widget.h",
            "struct Widget {\n  enum State { Ready };\n  Widget();\n  void run();\n};\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"include/Widget.h:Widget:method:cpp".to_string()));
        assert!(keys.contains(&"include/Widget.h:run:function:cpp".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reads_utf16le_powershell_files_with_bom() {
        let root = temp_workspace_root("utf16-powershell");
        let mut content = vec![0xFF, 0xFE];
        content.extend(
            "function Invoke-Thing { }\r\nfunction Get-OtherThing { }\r\n"
                .encode_utf16()
                .flat_map(|value| value.to_le_bytes()),
        );
        write_file_bytes(&root, "scripts/App.ps1", &content);

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"scripts/App.ps1:Invoke-Thing:function:powershell".to_string()));
        assert!(keys.contains(&"scripts/App.ps1:Get-OtherThing:function:powershell".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_qualified_and_structured_return_type_signatures() {
        let root = temp_workspace_root("qualified-return-types");
        write_file(
            &root,
            "src/Qualified.java",
            "import java.util.Map;\npublic class Qualified {\n  public static Map.Entry<String, String> nextEntry() { return null; }\n}\n",
        );
        write_file(
            &root,
            "src/Qualified.cs",
            "using System.Threading.Tasks;\npublic class Qualified {\n  public System.Threading.Tasks.Task RunAsync() { return Task.CompletedTask; }\n  public (int count, string label) BuildTuple() { return (1, \"ok\"); }\n}\n",
        );
        write_file(
            &root,
            "src/qualified.dart",
            "Map<String, int> buildMap() => {\"count\": 1};\n({int a, int b}) buildRecord() => (a: 1, b: 2);\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"src/Qualified.java:nextEntry:function:java".to_string()));
        assert!(keys.contains(&"src/Qualified.cs:RunAsync:function:csharp".to_string()));
        assert!(keys.contains(&"src/Qualified.cs:BuildTuple:function:csharp".to_string()));
        assert!(keys.contains(&"src/qualified.dart:buildMap:function:dart".to_string()));
        assert!(keys.contains(&"src/qualified.dart:buildRecord:function:dart".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_csharp_generic_methods_and_swift_attributed_functions() {
        let root = temp_workspace_root("generic-csharp-swift-attributes");
        write_file(
            &root,
            "src/GenericMethods.cs",
            "using System;\nusing System.Collections.Generic;\nusing System.Threading.Tasks;\npublic class GenericMethods {\n  public Task<TValue> CreateAsync<TValue>() where TValue : new() => Task.FromResult(new TValue());\n  internal Dictionary<TKey, TValue> Map<TKey, TValue>(IEnumerable<TValue> values) where TKey : notnull where TValue : class {\n    return new Dictionary<TKey, TValue>();\n  }\n}\n",
        );
        write_file(
            &root,
            "src/AppDelegate.swift",
            "final class AppDelegate {\n  @IBAction private func save(_ sender: Any) {}\n  @objc func applicationDidBecomeActive(_ notification: Notification) {}\n}\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"src/GenericMethods.cs:CreateAsync:function:csharp".to_string()));
        assert!(keys.contains(&"src/GenericMethods.cs:Map:function:csharp".to_string()));
        assert!(keys.contains(&"src/AppDelegate.swift:save:function:swift".to_string()));
        assert!(keys.contains(
            &"src/AppDelegate.swift:applicationDidBecomeActive:function:swift".to_string()
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_kotlin_same_line_annotated_functions() {
        let root = temp_workspace_root("kotlin-annotated-functions");
        write_file(
            &root,
            "src/Preview.kt",
            "@Composable fun Greeting() {}\n@Preview private fun CardPreview() {}\n@receiver:Composable fun String.renderGreeting() {}\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"src/Preview.kt:Greeting:function:kotlin".to_string()));
        assert!(keys.contains(&"src/Preview.kt:CardPreview:function:kotlin".to_string()));
        assert!(keys.contains(&"src/Preview.kt:renderGreeting:function:kotlin".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skips_false_positive_cpp_and_ml_value_symbols() {
        let root = temp_workspace_root("false-positive-symbols");
        write_file(
            &root,
            "src/app.cpp",
            "void render_page(int value);\nWidget buildWidget(Widget);\nvoid run() {\n  std::string name(\"x\");\n  Foo bar(arg);\n  Widget widget(factory());\n}\n",
        );
        write_file(
            &root,
            "src/app.ml",
            "let answer = 42\nlet config = load ()\nlet render_page value = value\nlet transform = fun value -> value\n",
        );
        write_file(
            &root,
            "src/App.fs",
            "type App() =\n    member this.Name = \"value\"\n    member this.Value with get() = 1\n    member this.Refresh value = value\n    member this.Invoke() = ()\nlet answer = 42\nlet config = load()\nlet renderPage value = value\nlet transform = fun value -> value\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"src/app.cpp:render_page:function:cpp".to_string()));
        assert!(keys.contains(&"src/app.cpp:buildWidget:function:cpp".to_string()));
        assert!(!keys.contains(&"src/app.cpp:name:function:cpp".to_string()));
        assert!(!keys.contains(&"src/app.cpp:bar:function:cpp".to_string()));
        assert!(!keys.contains(&"src/app.cpp:widget:function:cpp".to_string()));

        assert!(keys.contains(&"src/app.ml:render_page:function:ocaml".to_string()));
        assert!(keys.contains(&"src/app.ml:transform:function:ocaml".to_string()));
        assert!(!keys.contains(&"src/app.ml:answer:function:ocaml".to_string()));
        assert!(!keys.contains(&"src/app.ml:config:function:ocaml".to_string()));

        assert!(keys.contains(&"src/App.fs:Refresh:method:fsharp".to_string()));
        assert!(keys.contains(&"src/App.fs:Invoke:method:fsharp".to_string()));
        assert!(keys.contains(&"src/App.fs:renderPage:function:fsharp".to_string()));
        assert!(keys.contains(&"src/App.fs:transform:function:fsharp".to_string()));
        assert!(!keys.contains(&"src/App.fs:Name:method:fsharp".to_string()));
        assert!(!keys.contains(&"src/App.fs:Value:method:fsharp".to_string()));
        assert!(!keys.contains(&"src/App.fs:answer:function:fsharp".to_string()));
        assert!(!keys.contains(&"src/App.fs:config:function:fsharp".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skips_false_positive_cpp_h_headers_and_non_callable_c_family_declarations() {
        let root = temp_workspace_root("c-family-non-callables");
        write_file(
            &root,
            "include/app.h",
            "class Foo;\ninline int render_page(int value) {\n  std::string name(\"x\");\n  Foo bar(arg);\n  return value;\n}\n",
        );
        write_file(
            &root,
            "src/app.c",
            "typedef void Handler(int);\nvoid render_c(int value) {}\n",
        );
        write_file(
            &root,
            "src/App.cs",
            "public delegate void Handler(int value);\npublic class App {\n  public static implicit operator string(App value) => string.Empty;\n  public static explicit operator bool(App value) => true;\n  public void Render(int value) {}\n}\n",
        );
        write_file(
            &root,
            "src/app.cpp",
            "struct Flag {\n  explicit operator bool() const { return true; }\n};\nbool render_cpp() { return true; }\n",
        );
        write_file(
            &root,
            "ios/Bridge.mm",
            "#import <Foundation/Foundation.h>\nstruct Flag {\n  explicit operator bool() const { return true; }\n};\nWidget factory();\nbool render_bridge() {\n  Widget widget(factory());\n  return true;\n}\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"include/app.h:render_page:function:cpp".to_string()));
        assert!(!keys.contains(&"include/app.h:name:function:cpp".to_string()));
        assert!(!keys.contains(&"include/app.h:bar:function:cpp".to_string()));

        assert!(keys.contains(&"src/app.c:render_c:function:c".to_string()));
        assert!(!keys.contains(&"src/app.c:Handler:function:c".to_string()));

        assert!(keys.contains(&"src/App.cs:Render:function:csharp".to_string()));
        assert!(!keys.contains(&"src/App.cs:Handler:function:csharp".to_string()));
        assert!(!keys.contains(&"src/App.cs:string:function:csharp".to_string()));
        assert!(!keys.contains(&"src/App.cs:bool:function:csharp".to_string()));

        assert!(keys.contains(&"src/app.cpp:render_cpp:function:cpp".to_string()));
        assert!(!keys.contains(&"src/app.cpp:bool:function:cpp".to_string()));

        assert!(keys.contains(&"ios/Bridge.mm:render_bridge:function:objective-cpp".to_string()));
        assert!(!keys.contains(&"ios/Bridge.mm:bool:function:objective-cpp".to_string()));
        assert!(!keys.contains(&"ios/Bridge.mm:widget:function:objective-cpp".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_common_c_family_signature_variants_and_skips_gradle_task_dsl() {
        let root = temp_workspace_root("c-family-signature-variants");
        write_file(
            &root,
            "include/render.hpp",
            "char *render(void);\nWidget &lookup();\nstruct View {\n  void draw() override;\n};\nauto ready() -> bool;\n",
        );
        write_file(
            &root,
            "build.gradle",
            "task hello(type: Exec) {}\ndef configureBuild() { }\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(keys.contains(&"include/render.hpp:render:function:cpp".to_string()));
        assert!(keys.contains(&"include/render.hpp:lookup:function:cpp".to_string()));
        assert!(keys.contains(&"include/render.hpp:draw:function:cpp".to_string()));
        assert!(keys.contains(&"include/render.hpp:ready:function:cpp".to_string()));
        assert!(keys.contains(&"build.gradle:configureBuild:function:groovy".to_string()));
        assert!(!keys.contains(&"build.gradle:hello:function:groovy".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skips_java_and_csharp_record_declarations() {
        let root = temp_workspace_root("record-declarations");
        write_file(
            &root,
            "src/User.java",
            "public record User(String name) {}\npublic class Factory {\n  public Record createRecord() { return null; }\n}\n",
        );
        write_file(
            &root,
            "src/User.cs",
            "public readonly record struct User(int Id);\npublic class Factory {\n  public Record CreateRecord() { return default; }\n}\n",
        );

        let symbols = list_workspace_callable_symbols(&root, MAX_WORKSPACE_SYMBOLS);
        let keys = symbols
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}",
                    entry.path, entry.symbol, entry.kind, entry.language
                )
            })
            .collect::<Vec<_>>();

        assert!(!keys.contains(&"src/User.java:User:function:java".to_string()));
        assert!(!keys.contains(&"src/User.cs:User:function:csharp".to_string()));
        assert!(keys.contains(&"src/User.java:createRecord:function:java".to_string()));
        assert!(keys.contains(&"src/User.cs:CreateRecord:function:csharp".to_string()));

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
