use std::{fs, process::Command};

#[test]
fn filters_cli_output_by_symbol_kind() {
    let path = std::env::temp_dir().join(format!(
        "treesitter-index-kind-filter-test-{}.ts",
        std::process::id()
    ));
    fs::write(
        &path,
        "const LIMIT = 10;\ntype Id = string;\nclass Service {}\nfunction load(): void {}\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_treesitter-index"))
        .args(["--kind", "fns,classes"])
        .arg(&path)
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("classes:"));
    assert!(stdout.contains("  Service ["));
    assert!(stdout.contains("fns:"));
    assert!(stdout.contains("load(): void"));
    assert!(!stdout.contains("consts:"));
    assert!(!stdout.contains("types:"));
}
