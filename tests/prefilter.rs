use std::{fs, process::Command};

#[test]
fn prefilter_threshold_and_precise_matching_override() {
    let root = std::env::temp_dir().join(format!(
        "treesitter-index-prefilter-test-{}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("unknown.txt"), "ignored\n").unwrap();
    for index in 0..11 {
        fs::write(
            root.join(format!("{index}.rs")),
            "use std::{collections::HashSet};\n",
        )
        .unwrap();
    }

    let cases: &[(&[&str], bool, usize)] = &[
        (&["-g", "!10.rs"], true, 10),
        (&[], false, 0),
        (&["--no-prefilter"], true, 11),
    ];
    let outputs: Vec<_> = cases
        .iter()
        .map(|(args, hide_rg, _)| {
            let mut command = Command::new(env!("CARGO_BIN_EXE_treesitter-index"));
            command
                .args(["--match-imports", "-e", "^std::collections::HashSet$"])
                .args(*args)
                .arg(&root);
            if *hide_rg {
                command.env("PATH", "");
            }
            command.output().unwrap()
        })
        .collect();
    fs::remove_dir_all(&root).unwrap();

    for ((args, _, expected), output) in cases.iter().zip(outputs) {
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        // The grouped import is searchable after extraction, but not in raw source.
        assert_eq!(
            stdout.matches("std::collections::HashSet").count(),
            *expected
        );
    }
}
