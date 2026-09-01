//! ```text
//! cargo run -p axonflow-authzen-codegen             # write src/authzen/types_gen.rs
//! cargo run -p axonflow-authzen-codegen -- --check  # fail if it is out of date
//! ```
//!
//! Run from the SDK root, or pass the root as the last argument.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|a| a == "--check");
    let root: PathBuf = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let rendered = match axonflow_authzen_codegen::render(&root) {
        Ok(s) => s,
        Err(e) => return fatal(&e.to_string()),
    };
    let out = root.join(axonflow_authzen_codegen::output_path());

    if check {
        let have = match std::fs::read_to_string(&out) {
            Ok(s) => s,
            Err(e) => return fatal(&format!("--check: reading {}: {e}", out.display())),
        };
        if have != rendered {
            return fatal(&format!(
                "--check: {} is not what {} generates.\nRegenerate it in the same change:\n  \
                 cargo run -p axonflow-authzen-codegen",
                axonflow_authzen_codegen::output_path(),
                axonflow_authzen_codegen::surface_path(),
            ));
        }
        println!("{} is current.", axonflow_authzen_codegen::output_path());
        return ExitCode::SUCCESS;
    }

    if let Err(e) = std::fs::write(&out, rendered) {
        return fatal(&format!("writing {}: {e}", out.display()));
    }
    println!("wrote {}", out.display());
    ExitCode::SUCCESS
}

fn fatal(msg: &str) -> ExitCode {
    eprintln!("gen-authzen-types: {msg}");
    ExitCode::FAILURE
}
