use std::process::Command;

fn main() {
    if let Some(commit) = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
    {
        println!("cargo:rustc-env=GIT_COMMIT={}", commit.trim());
    }
}
