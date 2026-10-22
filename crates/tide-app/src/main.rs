#![cfg_attr(
    all(windows, not(debug_assertions), feature = "desktop"),
    windows_subsystem = "windows"
)]
use anyhow::{bail, Result};
#[cfg(feature = "desktop")]
mod desktop;
mod update;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use tide_core::{Config, Folder};
use tide_git::GitBackend;

#[derive(Parser)]
#[command(name = "tide", version, about = "Tide — Git 文档同步")]
struct Cli {
    /// Config file; defaults to the current user's application config directory.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Open the desktop window and background sync service.
    Gui {
        #[arg(long)]
        background: bool,
        #[arg(long, hide = true)]
        smoke_output: Option<PathBuf>,
    },
    /// Create a default configuration without overwriting an existing one.
    Init,
    /// Register a folder. Does not read/write the remote or sync documents.
    Add {
        name: String,
        path: PathBuf,
        remote: String,
        #[arg(long, default_value = "main")]
        branch: String,
        #[arg(long)]
        git_dir: Option<PathBuf>,
        #[arg(long)]
        ssh_key: Option<PathBuf>,
    },
    /// List configuration. Tokens in remote URLs are prohibited.
    List,
    /// Test connectivity and preview file counts, sizes and overlapping changes.
    Plan {
        folder: String,
    },
    /// Stop scheduling a folder.
    Pause {
        folder: String,
    },
    /// Resume a folder. Running watch/desktop processes reload configuration.
    Resume {
        folder: String,
    },
    /// Remove configuration only. Documents and all Git history remain on disk.
    Remove {
        folder: String,
    },
    /// Sync one folder, or all unpaused folders if none is specified.
    Sync {
        folder: Option<String>,
        /// Required only for first association of independent local and remote histories.
        #[arg(long)]
        confirm_initial_merge: bool,
    },
    /// Run event-driven file watching and interval scheduling in the foreground.
    Watch,
    /// Read at most 50 history entries; --skip selects the next page.
    History {
        folder: String,
        #[arg(long, default_value_t = 0)]
        skip: usize,
    },
    Diff {
        folder: String,
        revision: String,
        #[arg(long)]
        path: Option<String>,
    },
    /// Create a new local commit restoring a revision; run sync to publish it.
    Restore {
        folder: String,
        revision: String,
        #[arg(long)]
        path: Option<String>,
    },
    /// Export a historical file. Destination must not exist.
    Export {
        folder: String,
        revision: String,
        path: String,
        destination: PathBuf,
    },
    Conflicts {
        folder: String,
    },
    Resolve {
        folder: String,
        path: String,
        #[arg(value_enum)]
        choice: Choice,
    },
    /// Preserve the current worktree and cancel an interrupted write. Remote is unchanged.
    Recover {
        folder: String,
    },
    /// Replace content by adding history. Local pushes immediately; remote restores locally.
    Overwrite {
        folder: String,
        #[arg(value_enum)]
        source: Choice,
        #[arg(long)]
        confirm_initial_merge: bool,
    },
    /// Show the last 1000 sync results, newest first.
    Runs,
    /// Export, inspect, or explicitly import portable theme/notification preferences.
    Preferences {
        #[command(subcommand)]
        command: PreferencesCommand,
    },
    /// Check GitHub Releases for a newer version, or download and run its installer.
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
}
#[derive(Subcommand)]
enum UpdateCommand {
    /// Report the current and latest published versions.
    Check,
    /// Download the newer installer, verify its SHA-256 and start the setup wizard.
    Install,
}
#[derive(Subcommand)]
enum PreferencesCommand {
    Export { destination: PathBuf },
    Inspect { source: PathBuf },
    Import { source: PathBuf },
}
#[derive(Clone, ValueEnum)]
enum Choice {
    Local,
    Remote,
}

fn main() {
    #[cfg(windows)]
    if std::env::args_os().len() > 1 {
        // CLI keeps working when the release executable uses the GUI subsystem.
        unsafe {
            windows_sys::Win32::System::Console::AttachConsole(u32::MAX);
        }
    }
    if let Err(error) = execute(Cli::parse()) {
        eprintln!("Tide: {error:#}");
        std::process::exit(1);
    }
}
fn json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn execute(cli: Cli) -> Result<()> {
    let path = match cli.config {
        Some(p) => std::path::absolute(p)?,
        None => Config::default_path()?,
    };
    match cli.command.unwrap_or(Command::Gui {
        background: false,
        smoke_output: None,
    }) {
        Command::Gui {
            background,
            smoke_output,
        } => {
            #[cfg(feature = "desktop")]
            desktop::run(path, background, smoke_output)?;
            #[cfg(not(feature = "desktop"))]
            {
                let _ = (background, smoke_output);
                bail!("desktop support is disabled; rebuild with --features desktop");
            }
        }
        Command::Init => {
            Config::init(&path)?;
            println!("Created {}", path.display());
        }
        Command::Add {
            name,
            path: folder_path,
            remote,
            branch,
            git_dir,
            ssh_key,
        } => {
            let folder_path = folder_path.canonicalize()?;
            if !folder_path.is_dir() {
                bail!("local path must be a directory");
            }
            Config::edit(&path, |c| {
                let mut folder = Folder::new(name, folder_path, remote);
                folder.branch = branch;
                folder.git_dir = git_dir.map(std::path::absolute).transpose()?;
                folder.ssh_key = ssh_key.map(std::path::absolute).transpose()?;
                if c.folders
                    .iter()
                    .any(|f| f.remote == folder.remote && f.branch == folder.branch)
                {
                    eprintln!("Warning: another folder uses this remote and branch; both will synchronize the same content.");
                }
                c.folders.push(folder);
                Ok(())
            })?;
            println!("Folder registered. Use sync to start; inspect both sides before confirming independent histories.");
        }
        Command::List => json(&Config::load(&path)?)?,
        Command::Plan { folder } => json(&tide_core::open_folder(&path, &folder)?.plan()?)?,
        Command::Pause { folder } => Config::edit(&path, |c| {
            let id = c.folder(&folder)?.id.clone();
            c.folders.iter_mut().find(|f| f.id == id).unwrap().paused = true;
            Ok(())
        })?,
        Command::Resume { folder } => Config::edit(&path, |c| {
            let id = c.folder(&folder)?.id.clone();
            c.folders.iter_mut().find(|f| f.id == id).unwrap().paused = false;
            Ok(())
        })?,
        Command::Remove { folder } => Config::edit(&path, |c| {
            let id = c.folder(&folder)?.id.clone();
            c.folders.retain(|f| f.id != id);
            Ok(())
        })?,
        Command::Sync {
            folder,
            confirm_initial_merge,
        } => {
            let config = Config::load(&path)?;
            let names = match folder {
                Some(f) => vec![config.folder(&f)?.id.clone()],
                None => config
                    .folders
                    .iter()
                    .filter(|f| !f.paused)
                    .map(|f| f.id.clone())
                    .collect(),
            };
            let mut failed = false;
            for name in names {
                let run = tide_core::sync_folder(&path, &name, confirm_initial_merge)?;
                failed |= !run.success;
                json(&run)?;
            }
            if failed {
                bail!("one or more folders require attention; see the results above");
            }
        }
        Command::Watch => tide_core::watch(path)?,
        Command::History { folder, skip } => {
            json(&tide_core::open_folder(&path, &folder)?.history(skip, 50)?)?
        }
        Command::Diff {
            folder,
            revision,
            path: file,
        } => print!(
            "{}",
            tide_core::open_folder(&path, &folder)?.diff(&revision, file.as_deref())?
        ),
        Command::Restore {
            folder,
            revision,
            path: file,
        } => {
            tide_core::open_folder(&path, &folder)?.restore(&revision, file.as_deref())?;
            println!("Restored as a new local commit. Run sync to publish.");
        }
        Command::Export {
            folder,
            revision,
            path: file,
            destination,
        } => tide_core::open_folder(&path, &folder)?.export(&revision, &file, &destination)?,
        Command::Conflicts { folder } => {
            json(&tide_core::open_folder(&path, &folder)?.pending_conflicts()?)?
        }
        Command::Resolve {
            folder,
            path: file,
            choice,
        } => tide_core::open_folder(&path, &folder)?
            .resolve(&file, matches!(choice, Choice::Local))?,
        Command::Recover { folder } => {
            let config = Config::load(&path)?;
            tide_git::LibgitBackend::open_for_recovery(
                config.folder(&folder)?.options(&config.app),
            )?
            .recover_keep_worktree()?;
            println!("Current worktree preserved; interrupted target remains in history. Review before syncing.");
        }
        Command::Overwrite {
            folder,
            source,
            confirm_initial_merge,
        } => json(
            &tide_core::open_folder(&path, &folder)?
                .overwrite(matches!(source, Choice::Local), confirm_initial_merge)?,
        )?,
        Command::Runs => json(&tide_core::runs(&path)?)?,
        Command::Preferences { command } => match command {
            PreferencesCommand::Export { destination } => {
                tide_core::Preferences::from_config(&Config::load(&path)?)
                    .export(&std::path::absolute(destination)?)?;
            }
            PreferencesCommand::Inspect { source } => {
                json(&tide_core::Preferences::load(&source)?)?
            }
            PreferencesCommand::Import { source } => {
                let preferences = tide_core::Preferences::load(&source)?;
                preferences.apply(&path)?;
                json(&preferences)?;
            }
        },
        Command::Update { command } => run_update(command)?,
    }
    Ok(())
}

#[cfg(windows)]
fn run_update(command: UpdateCommand) -> Result<()> {
    #[derive(serde::Serialize)]
    struct Status {
        current: String,
        latest: Option<update::Release>,
        installed: bool,
    }
    let latest = update::check()?;
    let installed = update::installed_dir().is_some();
    match command {
        UpdateCommand::Check => json(&Status {
            current: update::current_version().to_string(),
            latest,
            installed,
        }),
        UpdateCommand::Install => {
            let Some(release) = latest else {
                println!("Tide {} is up to date.", update::current_version());
                return Ok(());
            };
            if !installed {
                bail!(
                    "this is a portable copy; download Tide {} from {}",
                    release.version,
                    release.page
                );
            }
            let installer = update::download(&release)?;
            println!("Verified {}", installer.path.display());
            update::launch_installer(&installer, update::InstallMode::Wizard)?;
            println!("Setup for Tide {} started.", release.version);
            Ok(())
        }
    }
}
#[cfg(not(windows))]
fn run_update(_: UpdateCommand) -> Result<()> {
    bail!(
        "automatic updates support the Windows installer; see {}/releases",
        update::REPOSITORY
    )
}
