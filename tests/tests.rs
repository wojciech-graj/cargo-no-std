use assert_cmd::{cargo_bin_cmd, Command};

fn command() -> Command {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("no-std");
    cmd
}

#[test]
fn no_std_success() {
    command()
        .arg("--manifest-path")
        .arg("tests/test-crate/Cargo.toml")
        .arg("--no-default-features")
        .assert()
        .success();
}

#[test]
fn no_std_failure() {
    command()
        .arg("--manifest-path")
        .arg("tests/test-crate/Cargo.toml")
        .assert()
        .failure();
}

#[test]
fn alloc_success() {
    command()
        .arg("--manifest-path")
        .arg("tests/test-crate/Cargo.toml")
        .arg("--no-default-features")
        .arg("--features")
        .arg("alloc")
        .arg("--alloc")
        .assert()
        .success();
}

#[test]
fn alloc_failure() {
    command()
        .arg("--manifest-path")
        .arg("tests/test-crate/Cargo.toml")
        .arg("--no-default-features")
        .arg("--features")
        .arg("alloc")
        .assert()
        .failure();
}

#[test]
fn std_target() {
    command()
        .arg("--manifest-path")
        .arg("tests/test-crate/Cargo.toml")
        .arg("--no-default-features")
        .arg("--target")
        .arg("x86_64-unknown-linux-gnu")
        .assert()
        .failure();
}

#[test]
fn version() {
    command().arg("--version").assert().success();
}
