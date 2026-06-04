// Copyright (C) 2026  Wojciech Graj
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

#![deny(
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::suspicious,
    clippy::complexity,
    clippy::perf,
    clippy::style
)]
#![allow(clippy::multiple_crate_versions)]

use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

use anyhow::{Error, Result, anyhow, bail, ensure};
use cargo_metadata::Package;
use cargo_toml::{Dependency, DependencyDetail, Manifest};
use clap::Parser;
use colored::Colorize;
use indicatif::{MultiProgress, ProgressBar};
use indicatif_log_bridge::LogWrapper;
use log::{error, info, warn};
use quote::{format_ident, quote};
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use serde_json::Value;
use toml_edit::DocumentMut;

const DEFAULT_TARGET: &str = "x86_64-unknown-none";

#[derive(Parser)]
#[command(
    bin_name = "cargo",
    version,
    disable_help_subcommand = true,
    styles = clap_cargo::style::CLAP_STYLING,
)]
enum Subcommand {
    #[command(name = "no-std", version, author, disable_version_flag = true)]
    NoStd(NoStd),
}

#[derive(Parser, Debug)]
#[command(styles = clap_cargo::style::CLAP_STYLING)]
#[allow(clippy::struct_excessive_bools)]
struct NoStd {
    #[command(flatten)]
    manifest: clap_cargo::Manifest,
    #[command(flatten)]
    workspace: clap_cargo::Workspace,
    #[command(flatten)]
    features: clap_cargo::Features,
    /// Target for which to check
    #[arg(long, default_value = DEFAULT_TARGET)]
    target: String,
    /// Allow usage of the alloc crate
    #[arg(long)]
    alloc: bool,
    /// Keep generated files
    #[arg(long)]
    keep: bool,
    /// Generate test crates in the workspace
    ///
    /// This option is useful for inhering workspace patches.
    /// NOTE: This will temporarily modify the workspace Cargo.toml file.
    #[arg(long)]
    in_workspace: bool,
    /// Use verbose output
    #[arg(short, long)]
    verbose: bool,
    /// Print version
    #[arg(short = 'V', long)]
    version: bool,
}

trait CommandExt {
    fn run(&mut self) -> Result<Output>;
}

impl CommandExt for Command {
    fn run(&mut self) -> Result<Output> {
        let output = self.output()?;
        ensure!(
            output.status.success(),
            "command returned non-zero exit code.\n{}",
            String::from_utf8_lossy(&output.stderr),
        );
        Ok(output)
    }
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    let logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).build();
    let level = logger.filter();
    let multi_progress = MultiProgress::new();
    LogWrapper::new(multi_progress.clone(), logger).try_init()?;
    log::set_max_level(level);

    let Subcommand::NoStd(args) = Subcommand::parse();

    if args.version {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if Command::new("rustup").arg("--version").run().is_ok() {
        let targets = String::from_utf8(
            Command::new("rustup")
                .arg("target")
                .arg("list")
                .arg("--installed")
                .run()?
                .stdout,
        )?;
        ensure!(
            targets.lines().any(|l| l == args.target),
            "target {} not installed.\nRun `rustup target add {}`, or use `--target` to select a different target",
            args.target,
            args.target
        );
    } else {
        warn!("could not locate rustup");
    }

    if args.target != DEFAULT_TARGET {
        match target_has_std(&args.target) {
            Ok(true) => bail!("selected target has std"),
            Err(e) if args.verbose => {
                warn!("could not check if selected target lacks std\n{e}");
            }
            _ => {}
        }
    }

    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));

    let mut metadata = args.manifest.metadata();
    args.features.forward_metadata(&mut metadata);
    let metadata = metadata.exec()?;
    let packages = args.workspace.partition_packages(&metadata).0;

    ensure!(!packages.is_empty(), "no packages to check");

    let check_crate_names = packages
        .iter()
        .map(|package| format!("{}-no-std-check-{}", package.name, std::process::id()))
        .collect::<Vec<_>>();

    let _cleanup = if args.in_workspace {
        let workspace_manifest_path = metadata.workspace_root.join("Cargo.toml");
        let workspace_lockfile_path = metadata.workspace_root.join("Cargo.lock");

        let mut manifest = fs::read_to_string(&workspace_manifest_path)?.parse::<DocumentMut>()?;
        let workspace = manifest
            .get_mut("workspace")
            .ok_or_else(|| anyhow!("workspace root missing 'workspace' item"))?
            .as_table_mut()
            .ok_or_else(|| anyhow!("workspace root 'workspace' item isn't a table"))?;
        let members = workspace
            .get_mut("members")
            .ok_or_else(|| anyhow!("workspace root missing 'workspace.members' item"))?
            .as_array_mut()
            .ok_or_else(|| anyhow!("workspace root 'workspace.members' item isn't an array"))?;
        for crate_ in &check_crate_names {
            members.push(crate_);
        }

        let cleanup_lockfile = workspace_lockfile_path
            .exists()
            .then(|| backup_file(workspace_lockfile_path))
            .transpose()?;
        let cleanup_manifest = backup_file(&workspace_manifest_path)?;

        fs::write(workspace_manifest_path, manifest.to_string().as_bytes())?;

        Some((cleanup_manifest, cleanup_lockfile))
    } else {
        None
    };

    let bars = packages
        .iter()
        .map(|package| {
            let bar = multi_progress.add(ProgressBar::new_spinner());
            bar.set_message(format!("  {}", package.name));
            bar
        })
        .collect::<Vec<_>>();

    let results = packages
        .par_iter()
        .zip(bars)
        .zip(check_crate_names)
        .map(|((package, bar), check_crate_name)| {
            const TICK_INTERVAL: Duration = Duration::from_millis(200);
            bar.enable_steady_tick(TICK_INTERVAL);
            let result = check_package(
                &check_crate_name,
                package,
                &args,
                &cargo,
                &metadata.workspace_root,
            );
            bar.finish_with_message(format!(
                "{} {}",
                match &result {
                    Ok(()) => "✓".green(),
                    Err(_) => "✗".red(),
                },
                package.name
            ));
            result
        })
        .collect::<Vec<_>>();

    let errors = packages
        .iter()
        .zip(results)
        .fold(String::new(), |acc, (package, result)| match result {
            Ok(()) => acc,
            Err(e) => acc + &format!("\n{}: {}", package.name.to_string().bold(), e),
        });

    if !errors.is_empty() {
        bail!("{errors}")
    }

    Ok(())
}

fn backup_file<P>(path: P) -> Result<RenameGuard<PathBuf, PathBuf>, Error>
where
    P: AsRef<Path>,
{
    let backup_path = path.as_ref().with_added_extension("bak");
    let cleanup = RenameGuard {
        from: path.as_ref().to_path_buf(),
        to: backup_path.clone(),
    };
    fs::copy(&path, &backup_path)?;
    info!("created backup {}", backup_path.to_string_lossy());
    Ok(cleanup)
}

fn check_package(
    check_crate_name: &str,
    package: &Package,
    args: &NoStd,
    cargo: impl AsRef<OsStr>,
    workspace_root: impl AsRef<Path>,
) -> Result<()> {
    let name = package.name.to_string();

    let path = if args.in_workspace {
        workspace_root.as_ref().join(check_crate_name)
    } else {
        tempfile::tempdir()?.keep()
    };

    let _cleanup = if args.keep {
        info!(
            "storing build files for {} at {}",
            name,
            path.to_string_lossy()
        );
        None
    } else {
        Some(RemoveDirAllGuard(&path))
    };

    fs::create_dir_all(path.join("src"))?;

    let alloc_section = args.alloc.then(|| {
        quote! {
            struct Allocator;

            unsafe impl core::alloc::GlobalAlloc for Allocator {
                unsafe fn alloc(&self, _: core::alloc::Layout) -> *mut u8 {
                    panic!()
                }

                unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {
                    panic!()
                }
            }

            #[global_allocator]
            static GLOBAL: Allocator = Allocator;
        }
    });
    let name_ident = format_ident!("{}", name.replace('-', "_"));
    let code = quote! {
        #![no_std]
        #![no_main]

        extern crate #name_ident;

        #alloc_section

        #[no_mangle]
        fn main() {}

        #[panic_handler]
        fn panic(_: &core::panic::PanicInfo) -> ! {
            loop {}
        }
    };

    fs::write(path.join("src/main.rs"), code.to_string())?;

    let dep = DependencyDetail {
        path: Some(
            package
                .manifest_path
                .parent()
                .ok_or_else(|| anyhow!("manifest lacks parent"))?
                .to_string(),
        ),
        default_features: !args.features.no_default_features,
        features: if args.features.all_features {
            package.features.keys().cloned().collect()
        } else {
            args.features.features.clone()
        },
        ..Default::default()
    };

    let manifest: Manifest<()> = Manifest {
        package: Some(cargo_toml::Package::new("no-std-check", "0.0.0")),
        dependencies: BTreeMap::from_iter([(name, Dependency::Detailed(Box::new(dep)))]),
        ..Default::default()
    };
    let manifest_path = path.join("Cargo.toml");
    fs::write(&manifest_path, toml::to_string(&manifest)?)?;

    Command::new(cargo)
        .arg("build")
        .arg("--manifest-path")
        .arg(manifest_path)
        .arg("--target")
        .arg(&args.target)
        .run()?;

    Ok(())
}

fn target_has_std(target: &str) -> Result<bool> {
    let output = Command::new("rustc")
        .arg("+nightly")
        .arg("-Z")
        .arg("unstable-options")
        .arg("--print")
        .arg("target-spec-json")
        .arg("--target")
        .arg(target)
        .run()?;
    let output = String::from_utf8(output.stdout)?;
    let target_spec: Value = serde_json::from_str(&output)?;
    let metadata = target_spec
        .get("metadata")
        .ok_or_else(|| anyhow!("missing metadata field"))?;
    let has_std = metadata
        .get("std")
        .ok_or_else(|| anyhow!("missing std field"))?;
    has_std
        .as_bool()
        .ok_or_else(|| anyhow!("std field is not bool"))
}

#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
struct RemoveDirAllGuard<P>(pub P)
where
    P: AsRef<Path>;

impl<P> Drop for RemoveDirAllGuard<P>
where
    P: AsRef<Path>,
{
    fn drop(&mut self) {
        if let Err(e) = fs::remove_dir_all(&self.0) {
            error!(
                "failed to delete directory {}: {}",
                self.0.as_ref().to_string_lossy(),
                e
            );
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
struct RenameGuard<P, Q>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
{
    pub from: P,
    pub to: Q,
}

impl<P, Q> Drop for RenameGuard<P, Q>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
{
    fn drop(&mut self) {
        if let Err(e) = fs::rename(&self.from, &self.to) {
            error!(
                "failed to rename {} to {}: {}",
                self.from.as_ref().to_string_lossy(),
                self.to.as_ref().to_string_lossy(),
                e
            );
        }
    }
}
