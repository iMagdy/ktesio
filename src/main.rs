mod cli;
mod discovery;
mod error;
mod git;
mod install_channel;
mod install_target;
mod lockfile;
mod manifest;
mod skill;
mod skills_sh;
mod ui;
mod update_check;

use clap::{CommandFactory, Parser, Subcommand};

const HELP_FOOTER: &str = concat!(
    "License: ",
    env!("CARGO_PKG_LICENSE"),
    "\nRepository: ",
    env!("CARGO_PKG_REPOSITORY")
);

const INIT_AFTER_HELP: &str = "\
Details:
  Creates a manifest with dependencies and publish fields. Existing
  .agents/skills directories are adopted as dependencies: known public skills
  are resolved to remote dependencies when possible, and unmatched skills stay
  as local path dependencies. Adoption reports lookup, clone, and fallback
  progress for each existing skill. If skills.json already exists, Ktesio
  leaves it untouched.

Example:
  kt init .";

const INSTALL_AFTER_HELP: &str = "\
Details:
  With no argument, installs every dependency declared in skills.json. With
  name:repo, adds one skill after the repo is fetched and copied successfully.
  With a bare repo URL or local path, reads published skills from that repo and lets you
  choose which skills to install. GitHub owner/repo shorthand resolves to an
  HTTPS clone URL by default; use --ssh to prefer SSH. Add /skill or --skill to
  install one published skill from a multi-skill repository.

Examples:
  kt install
  kt install docs:https://github.com/example/agent-docs.git
  kt install docs:hashicorp/agent-skills/run-acceptance-tests
  kt install --all https://github.com/example/agent-docs.git";

const SEARCH_AFTER_HELP: &str = "\
Details:
  Searches skills.sh for public skill listings and prints install targets that
  Ktesio can clone from git. Ktesio uses the skills.sh public API responsibly,
  respects rate limits with bounded retries, and will use the documented
  authenticated API when KTESIO_SKILLS_SH_API_KEY is configured.

Examples:
  kt search tests
  kt search \"react native\" --limit 10
  kt search tests --install";

const UPGRADE_AFTER_HELP: &str = "\
Details:
  Fetches latest upstream commits, checks out each skill's default branch, and
  updates skills.lock. Per-skill failures are reported after the rest run.

Example:
  kt upgrade";

const SELF_UPDATE_AFTER_HELP: &str = "\
Details:
  Updates the kt binary using the current install channel. Homebrew installs run
  brew upgrade, Cargo installs run cargo install --force, and manual binary
  installs download and verify the latest GitHub Release archive.

Example:
  kt self-update";

const PUBLISH_AFTER_HELP: &str = "\
Details:
  Publishes local skill paths from this repo so other projects can install
  them. Use publish add to expose a local file or directory directly.

Example:
  kt publish
  kt publish add docs skills/docs";

const LIST_AFTER_HELP: &str = "\
Details:
  Shows each known skill with repo, commit, and status. Statuses include
  installed, missing, not locked, and orphaned.

Example:
  kt list";

const SHOW_AFTER_HELP: &str = "\
Details:
  Shows the repo URL, locked commit, local installation path, and current status
  for one skill.

Example:
  kt show docs";

const DOCTOR_AFTER_HELP: &str = "\
Details:
  Validates skills.json, skills.lock, installed skill directories, published local paths,
  orphaned lock entries, and git availability.

Example:
  kt doctor";

const UNINSTALL_AFTER_HELP: &str = "\
Details:
  Removes one skill from skills.json, skills.lock, and .agents/skills. The
  remove subcommand is an alias for uninstall.

Examples:
  kt uninstall docs
  kt remove docs";

#[derive(Parser)]
#[command(
    name = "kt",
    version,
    about = "Agentic skills package manager",
    after_help = HELP_FOOTER
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new skills.json manifest
    #[command(
        about = "Initialize a new skills.json manifest",
        after_help = INIT_AFTER_HELP
    )]
    Init {
        /// Directory path where skills.json will be created
        path: String,
    },
    /// Install skills from skills.json
    #[command(
        about = "Install skills from skills.json",
        after_help = INSTALL_AFTER_HELP
    )]
    Install {
        /// Install every discovered published skill from a repo target
        #[arg(long)]
        all: bool,
        /// Resolve GitHub owner/repo shorthand to an SSH clone URL
        #[arg(long)]
        ssh: bool,
        /// Install one named source published skill from the target repo
        #[arg(long)]
        skill: Option<String>,
        /// Accept safe defaults for prompts
        #[arg(long)]
        yes: bool,
        /// Fail instead of prompting for interactive choices
        #[arg(long = "no-input")]
        no_input: bool,
        /// Optional: skill name and repo URL (format: name:url)
        target: Option<String>,
    },
    /// Search public skill listings
    #[command(
        about = "Search public skill listings",
        after_help = SEARCH_AFTER_HELP
    )]
    Search {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Maximum number of results to return
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Select and install one result after searching
        #[arg(long)]
        install: bool,
        /// Fail instead of prompting for interactive choices
        #[arg(long = "no-input")]
        no_input: bool,
        /// Search query
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
    },
    /// Upgrade all installed skills to latest versions
    #[command(
        about = "Upgrade all installed skills to latest versions",
        after_help = UPGRADE_AFTER_HELP
    )]
    Upgrade,
    /// Update the kt binary
    #[command(
        name = "self-update",
        about = "Update the kt binary",
        after_help = SELF_UPDATE_AFTER_HELP
    )]
    SelfUpdate,
    /// Publish local skills from this repo
    #[command(
        about = "Publish local skills from this repo",
        after_help = PUBLISH_AFTER_HELP
    )]
    Publish {
        #[command(subcommand)]
        command: Option<PublishCommands>,
    },
    /// List installed skills
    #[command(
        about = "List installed skills",
        after_help = LIST_AFTER_HELP
    )]
    List {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Show details for a specific skill
    #[command(
        about = "Show details for a specific skill",
        after_help = SHOW_AFTER_HELP
    )]
    Show {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Name of the skill to inspect
        package_name: String,
    },
    /// Validate project skill state
    #[command(
        about = "Validate project skill state",
        after_help = DOCTOR_AFTER_HELP
    )]
    Doctor,
    /// Remove a skill from the project
    #[command(
        alias = "remove",
        about = "Remove a skill from the project",
        after_help = UNINSTALL_AFTER_HELP
    )]
    Uninstall {
        /// Name of the skill to remove
        package_name: String,
    },
}

#[derive(Subcommand)]
enum PublishCommands {
    /// Add or update one published local skill in skills.json
    Add {
        /// Published skill name
        skill: String,
        /// Local file or directory path to publish
        path: String,
    },
}

#[cfg(not(tarpaulin_include))]
fn main() {
    if let Err(err) = run_cli() {
        ui::error(err);
        std::process::exit(1);
    }
}

#[cfg(not(tarpaulin_include))]
fn run_cli() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    if should_check_for_updates(&cli.command) {
        if let Some(notice) = update_check::maybe_notice() {
            ui::update_notice(&notice.current_version, &notice.latest_version);
        }
    }

    match cli.command {
        Some(Commands::Init { path }) => cli::init::run(&path),
        Some(Commands::Install {
            all,
            ssh,
            skill,
            yes,
            no_input,
            target,
        }) => cli::install::run_with_options(
            target.as_deref(),
            cli::install::InstallOptions {
                all,
                yes,
                no_input,
                ssh,
                skill,
            },
        ),
        Some(Commands::Search {
            json,
            limit,
            install,
            no_input,
            query,
        }) => cli::search::run_with_options(
            &query.join(" "),
            cli::search::SearchOptions {
                json,
                limit,
                install,
                no_input,
            },
        ),
        Some(Commands::Upgrade) => cli::upgrade::run(),
        Some(Commands::SelfUpdate) => cli::self_update::run(),
        Some(Commands::Publish { command }) => match command {
            Some(PublishCommands::Add { skill, path }) => cli::publish::run_add(&skill, &path),
            None => cli::publish::run(),
        },
        Some(Commands::List { json }) => cli::list::run_with_options(json),
        Some(Commands::Show { json, package_name }) => {
            cli::show::run_with_options(&package_name, json)
        }
        Some(Commands::Doctor) => cli::doctor::run(),
        Some(Commands::Uninstall { package_name }) => cli::uninstall::run(&package_name),
        None => {
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
    }
}

fn should_check_for_updates(command: &Option<Commands>) -> bool {
    matches!(command, Some(command) if !matches!(command, Commands::SelfUpdate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_struct_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn test_cli_subcommands_exist() {
        let cmd = Cli::command();
        assert!(cmd.find_subcommand("init").is_some());
        assert!(cmd.find_subcommand("install").is_some());
        assert!(cmd.find_subcommand("search").is_some());
        assert!(cmd.find_subcommand("upgrade").is_some());
        assert!(cmd.find_subcommand("self-update").is_some());
        assert!(cmd.find_subcommand("publish").is_some());
        assert!(cmd.find_subcommand("list").is_some());
        assert!(cmd.find_subcommand("show").is_some());
        assert!(cmd.find_subcommand("doctor").is_some());
        assert!(cmd.find_subcommand("uninstall").is_some());
        assert!(cmd.find_subcommand("remove").is_some());
    }

    #[test]
    fn test_cli_help_includes_license_and_repository() {
        let help = Cli::command().render_help().to_string();
        assert!(help.contains("License: PolyForm-Noncommercial-1.0.0"));
        assert!(help.contains("Repository: https://github.com/iMagdy/ktesio"));
    }

    #[test]
    fn test_cli_without_subcommand_is_allowed_for_help_display() {
        let cli = Cli::try_parse_from(["kt"]).expect("bare kt should parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_subcommand_help_includes_details_and_examples() {
        for (command, detail) in [
            ("init", "Creates a manifest with dependencies"),
            ("install", "installs every dependency"),
            ("upgrade", "Fetches latest upstream commits"),
            ("self-update", "Updates the kt binary"),
            ("publish", "Publishes local skill paths"),
            ("list", "Shows each known skill"),
            ("show", "Shows the repo URL"),
            ("doctor", "Validates skills.json"),
            ("uninstall", "Removes one skill"),
        ] {
            let mut cmd = Cli::command();
            let help = cmd
                .find_subcommand_mut(command)
                .expect("subcommand should exist")
                .render_help()
                .to_string();
            assert!(help.contains(detail), "{} help missing detail", command);
            assert!(help.contains("Example"), "{} help missing example", command);
        }
    }

    #[test]
    fn test_self_update_skips_passive_update_check() {
        let cli = Cli::try_parse_from(["kt", "self-update"]).expect("self-update should parse");

        assert!(!should_check_for_updates(&cli.command));
    }
}
