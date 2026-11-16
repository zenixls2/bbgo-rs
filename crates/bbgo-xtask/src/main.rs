use std::{path::Path, process::Command};

fn main() {
    let mut args = std::env::args().skip(1);
    let task = args.next().unwrap_or_default();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut command = match task.as_str() {
        "sql-swarm-init" => {
            let mut command = Command::new("docker");
            command.args(["swarm", "init"]);
            command
        }
        "test-sql-swarm" => {
            let mut command = Command::new("bash");
            command.arg(root.join("scripts/test-sql-swarm.sh"));
            command
        }
        "test-sql-local" => {
            let mut command = Command::new("python3");
            command.arg(root.join("scripts/test_sql_local.py"));
            command
        }
        _ => {
            eprintln!(
                "Usage: cargo sql-swarm-init | cargo test-sql-swarm [cargo test options] | cargo test-sql-local --help"
            );
            std::process::exit(2);
        }
    };
    let status = command.current_dir(root).args(args).status();
    match status {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("Cannot run {task}: {error}");
            std::process::exit(1);
        }
    }
}
