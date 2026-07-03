use std::path::Path;

use dialoguer::Select;

use crate::error::SearchFailed;
use crate::skills_sh::{self, SkillSearchResult};
use crate::ui;

#[derive(Debug, Clone, Copy, Default)]
pub struct SearchOptions {
    pub json: bool,
    pub limit: usize,
    pub install: bool,
    pub no_input: bool,
}

#[cfg(not(tarpaulin_include))]
pub fn run_with_options(
    query: &str,
    options: SearchOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let project_root = std::env::current_dir()?;
    run_in_with_options(&project_root, query, options)
}

#[cfg(not(tarpaulin_include))]
pub(crate) fn run_in_with_options(
    project_root: &Path,
    query: &str,
    options: SearchOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    run_in_with_dependencies(
        project_root,
        query,
        options,
        |query, limit, notify| skills_sh::search(query, limit, notify),
        |project_root, target, no_input| {
            crate::cli::install::run_in_with_options(
                project_root,
                Some(target),
                crate::cli::install::InstallOptions {
                    yes: true,
                    no_input,
                    ..Default::default()
                },
            )
        },
    )
}

fn run_in_with_dependencies<S, I>(
    project_root: &Path,
    query: &str,
    options: SearchOptions,
    mut search: S,
    mut install: I,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: FnMut(
        &str,
        usize,
        &mut dyn FnMut(String),
    ) -> Result<Vec<SkillSearchResult>, Box<dyn std::error::Error>>,
    I: FnMut(&Path, &str, bool) -> Result<(), Box<dyn std::error::Error>>,
{
    if options.json && options.install {
        return Err(SearchFailed {
            message: "Cannot combine --json and --install".to_string(),
        }
        .into());
    }

    if !options.json {
        ui::info("Searching skills.sh public listings (rate-limited, with retries).");
    }

    let mut notify = |message| ui::warning(message);
    let results = search(query, options.limit, &mut notify)?;

    if options.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }

    print_results(&results);

    if options.install {
        install_selected(project_root, &results, options.no_input, &mut install)?;
    }

    Ok(())
}

fn print_results(results: &[SkillSearchResult]) {
    if results.is_empty() {
        ui::info("No skills found.");
        return;
    }

    let columns = [
        ui::TableColumn::new("Skill", 16, 30),
        ui::TableColumn::new("Source", 16, 34),
        ui::TableColumn::new("Installs", 8, 9).right(),
        ui::TableColumn::new("Install", 18, 54),
    ];
    let rows = results
        .iter()
        .map(|result| {
            let install = result
                .install_target
                .as_deref()
                .map(|target| format!("kt install {target}"))
                .unwrap_or_else(|| "not installable yet".to_string());
            let install_cell = if result.installable {
                ui::TableCell::command(install)
            } else {
                ui::TableCell::muted(install)
            };

            vec![
                ui::TableCell::skill(result.name.as_str()),
                ui::TableCell::muted(ui::compact_source(&result.source)),
                ui::TableCell::number(result.installs.to_string()),
                install_cell,
            ]
        })
        .collect::<Vec<_>>();
    ui::print_table("Search results", &columns, &rows);
}

fn install_selected(
    project_root: &Path,
    results: &[SkillSearchResult],
    no_input: bool,
    install: &mut impl FnMut(&Path, &str, bool) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let installable = results
        .iter()
        .filter(|result| result.installable)
        .collect::<Vec<_>>();

    if installable.is_empty() {
        return Err(SearchFailed {
            message: "No installable GitHub-backed skills were found.".to_string(),
        }
        .into());
    }

    let selected = if installable.len() == 1 {
        installable[0]
    } else if no_input {
        return Err(SearchFailed {
            message: "Multiple installable skills found; rerun without --no-input to select one."
                .to_string(),
        }
        .into());
    } else {
        let index = prompt_install_selection(&installable)?;
        installable[index]
    };

    let target = selected
        .install_target
        .as_deref()
        .ok_or_else(|| SearchFailed {
            message: "Selected skill is not installable yet".to_string(),
        })?;
    install(project_root, target, no_input)
}

#[cfg(not(tarpaulin_include))]
fn prompt_install_selection(
    installable: &[&SkillSearchResult],
) -> Result<usize, Box<dyn std::error::Error>> {
    let labels = installable
        .iter()
        .map(|result| {
            let target = result
                .install_target
                .as_deref()
                .unwrap_or("not installable");
            format!("{} ({target})", result.name)
        })
        .collect::<Vec<_>>();
    Select::new()
        .with_prompt("Select skill to install")
        .items(&labels)
        .default(0)
        .interact_opt()?
        .ok_or_else(|| {
            SearchFailed {
                message: "Installation cancelled".to_string(),
            }
            .into()
        })
}

#[cfg(tarpaulin_include)]
fn prompt_install_selection(
    _installable: &[&SkillSearchResult],
) -> Result<usize, Box<dyn std::error::Error>> {
    Err(SearchFailed {
        message: "Interactive search installation is disabled during coverage runs".to_string(),
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(name: &str, install_target: Option<&str>) -> SkillSearchResult {
        SkillSearchResult {
            id: format!("example/repo/{name}"),
            name: name.to_string(),
            source: "example/repo".to_string(),
            skill: name.to_string(),
            repo: Some("https://github.com/example/repo.git".to_string()),
            installs: 42,
            url: Some(format!("https://skills.sh/example/repo/{name}")),
            install_target: install_target.map(str::to_string),
            installable: install_target.is_some(),
        }
    }

    #[test]
    fn test_print_results_handles_empty() {
        print_results(&[]);
    }

    #[test]
    fn test_print_results_handles_installable_and_unsupported_results() {
        print_results(&[
            result("docs", Some("example/repo/docs")),
            SkillSearchResult {
                source: "external-catalog".to_string(),
                repo: None,
                installable: false,
                install_target: None,
                ..result("external", None)
            },
        ]);
    }

    #[test]
    fn test_run_in_rejects_json_install_before_searching() {
        let mut searched = false;
        let mut installed = false;

        let result = run_in_with_dependencies(
            Path::new("."),
            "docs",
            SearchOptions {
                json: true,
                install: true,
                limit: 10,
                no_input: false,
            },
            |_, _, _| {
                searched = true;
                Ok(Vec::new())
            },
            |_, _, _| {
                installed = true;
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(!searched);
        assert!(!installed);
    }

    #[test]
    fn test_run_in_prints_json_without_installing() {
        let mut seen_query = String::new();
        let mut seen_limit = 0;
        let mut installed = false;

        let result = run_in_with_dependencies(
            Path::new("."),
            "docs",
            SearchOptions {
                json: true,
                limit: 7,
                install: false,
                no_input: false,
            },
            |query, limit, notify| {
                seen_query = query.to_string();
                seen_limit = limit;
                notify("retry message".to_string());
                Ok(vec![result("docs", Some("example/repo/docs"))])
            },
            |_, _, _| {
                installed = true;
                Ok(())
            },
        );

        assert!(result.is_ok());
        assert_eq!(seen_query, "docs");
        assert_eq!(seen_limit, 7);
        assert!(!installed);
    }

    #[test]
    fn test_run_in_installs_single_installable_result() {
        let mut installed_target = String::new();
        let mut installed_no_input = false;

        let result = run_in_with_dependencies(
            Path::new("/project"),
            "docs",
            SearchOptions {
                install: true,
                no_input: true,
                limit: 3,
                json: false,
            },
            |_, _, _| {
                Ok(vec![
                    result("external", None),
                    result("docs", Some("example/repo/docs")),
                ])
            },
            |project_root, target, no_input| {
                assert_eq!(project_root, Path::new("/project"));
                installed_target = target.to_string();
                installed_no_input = no_input;
                Ok(())
            },
        );

        assert!(result.is_ok());
        assert_eq!(installed_target, "example/repo/docs");
        assert!(installed_no_input);
    }

    #[test]
    fn test_install_selected_rejects_empty_and_ambiguous_results() {
        let mut installed = false;
        let empty = install_selected(Path::new("."), &[], false, &mut |_, _, _| {
            installed = true;
            Ok(())
        });
        assert!(empty.is_err());
        assert!(!installed);

        let ambiguous = install_selected(
            Path::new("."),
            &[
                result("alpha", Some("example/repo/alpha")),
                result("beta", Some("example/repo/beta")),
            ],
            true,
            &mut |_, _, _| {
                installed = true;
                Ok(())
            },
        );
        assert!(ambiguous.is_err());
        assert!(!installed);
    }

    #[test]
    fn test_install_selected_rejects_inconsistent_result() {
        let mut inconsistent = result("docs", None);
        inconsistent.installable = true;

        let result = install_selected(
            Path::new("."),
            &[inconsistent],
            false,
            &mut |_, _, _| Ok(()),
        );

        assert!(result.is_err());
    }

    #[cfg(tarpaulin_include)]
    #[test]
    fn test_install_selected_interactive_prompt_is_disabled_for_coverage() {
        let result = install_selected(
            Path::new("."),
            &[
                result("alpha", Some("example/repo/alpha")),
                result("beta", Some("example/repo/beta")),
            ],
            false,
            &mut |_, _, _| Ok(()),
        );

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Interactive search installation is disabled"));
    }
}
