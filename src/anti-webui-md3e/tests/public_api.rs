//! This crate is what a crate of components made outside AntiWebUI would be, so it may only use
//! what programs get: the prelude, the standard widgets and the check for shaders in tests. See
//! `docs/design/33-customization.md`.

use std::path::Path;

const ALLOWED: [&str; 3] = [
    "anti_webui::prelude",
    "anti_webui::widgets",
    "anti_webui::wgpu::check_shader",
];

fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn only_what_programs_get_is_used() {
    let mut files = Vec::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(!files.is_empty());
    let mut wrong = Vec::new();
    for file in files {
        let source = std::fs::read_to_string(&file).unwrap();
        for (n, line) in source.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for (at, _) in code.match_indices("anti_webui") {
                let path: String = code[at..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                    .collect();
                // The crate of icons is a crate of its own, made for programs.
                if path.starts_with("anti_webui_symbols_outlined") {
                    continue;
                }
                if !ALLOWED.iter().any(|a| path.starts_with(a)) {
                    wrong.push(format!("{}:{}: {path}", file.display(), n + 1));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "not public:\n{}", wrong.join("\n"));
}
