# cargo-no-std

cargo-no-std is a Cargo subcommand that checks whether your Rust crate is compatible with `no_std` environments, optionally allowing usage of the `alloc` crate.

## Usage

### Installation

```sh
cargo install cargo-no-std
```

### Command line

```
Usage: cargo no-std [OPTIONS]

Options:
      --manifest-path <PATH>  Path to Cargo.toml
  -p, --package <SPEC>        Package to process (see `cargo help pkgid`)
      --workspace             Process all packages in the workspace
      --exclude <SPEC>        Exclude packages from being processed
      --all-features          Activate all available features
      --no-default-features   Do not activate the `default` feature
  -F, --features <FEATURES>   Space-separated list of features to activate
      --target <TARGET>       Target for which to check [default: x86_64-unknown-none]
      --alloc                 Allow usage of the alloc crate
  -v, --verbose               Use verbose output
  -V, --version               Print version
  -h, --help                  Print help
```
