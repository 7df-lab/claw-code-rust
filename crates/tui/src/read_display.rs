use std::path::Path;

use devo_protocol::parse_command::ParsedCommand;

fn relativize_path_str(path_str: &str, cwd: &Path) -> String {
    // `Path::is_absolute()` behaves differently across platforms for unix-style
    // absolute paths (e.g. `/workspace/src/file.rs` on Windows). For display
    // purposes, normalize this case via string prefix matching first.
    let path_s = path_str.replace('\\', "/");
    let cwd_s = cwd.to_string_lossy().replace('\\', "/");
    if path_s.starts_with('/') {
        if path_s == cwd_s {
            // Path equals cwd — show just the folder name
            return cwd.file_name().map_or_else(
                || path_str.to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
        }
        let prefix = format!("{cwd_s}/");
        if let Some(rest) = path_s.strip_prefix(&prefix) {
            return rest.to_string();
        }
    }

    let p = Path::new(path_str);
    if p.is_absolute() {
        match p.strip_prefix(cwd) {
            Ok(relative) if relative.as_os_str().is_empty() => {
                // Path equals cwd — show just the folder name
                cwd.file_name().map_or_else(
                    || path_str.to_string(),
                    |name| name.to_string_lossy().into_owned(),
                )
            }
            Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
            Err(_) => path_str.to_string(),
        }
    } else {
        path_str.to_string()
    }
}

pub(crate) fn normalize_read_actions(actions: &mut [ParsedCommand], cwd: &Path) {
    for action in actions {
        match action {
            ParsedCommand::Read { name, path, .. } => {
                let (base_name, suffix) = name
                    .rsplit_once(" L:")
                    .map_or((name.as_str(), ""), |(path, range)| (path, range));
                let path_s = path.to_string_lossy().replace('\\', "/");
                let cwd_s = cwd.to_string_lossy().replace('\\', "/");
                let display_path = if path_s.starts_with('/') {
                    if path_s == cwd_s {
                        cwd.file_name().map_or_else(
                            || path.to_string_lossy().into_owned(),
                            |name| name.to_string_lossy().into_owned(),
                        )
                    } else if let Some(rest) = path_s.strip_prefix(&format!("{cwd_s}/")) {
                        rest.to_string()
                    } else {
                        path.to_string_lossy().into_owned()
                    }
                } else if path.is_absolute() {
                    match path.strip_prefix(cwd) {
                        Ok(relative) if relative.as_os_str().is_empty() => {
                            cwd.file_name().map_or_else(
                                || path.to_string_lossy().into_owned(),
                                |name| name.to_string_lossy().into_owned(),
                            )
                        }
                        Ok(relative) => relative.to_string_lossy().into_owned(),
                        Err(_) => path.to_string_lossy().into_owned(),
                    }
                } else if !path.as_os_str().is_empty() {
                    path.to_string_lossy().into_owned()
                } else {
                    base_name.to_string()
                };
                let display_path = display_path.replace('\\', "/");
                *name = if suffix.is_empty() {
                    display_path
                } else {
                    format!("{display_path} L:{suffix}")
                };
            }
            ParsedCommand::Search {
                path: Some(path), ..
            }
            | ParsedCommand::ListFiles {
                path: Some(path), ..
            } => {
                let relativized = relativize_path_str(path, cwd);
                if relativized != *path {
                    *path = relativized;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn normalizes_absolute_read_path_to_cwd_relative_path() {
        let mut actions = vec![ParsedCommand::Read {
            cmd: "read".to_string(),
            name: "/workspace/src/query.rs L:20-29".to_string(),
            path: PathBuf::from("/workspace/src/query.rs"),
        }];

        normalize_read_actions(&mut actions, Path::new("/workspace"));

        assert_eq!(
            actions,
            vec![ParsedCommand::Read {
                cmd: "read".to_string(),
                name: "src/query.rs L:20-29".to_string(),
                path: PathBuf::from("/workspace/src/query.rs"),
            }]
        );
    }

    #[test]
    fn keeps_absolute_read_path_outside_cwd() {
        let mut actions = vec![ParsedCommand::Read {
            cmd: "read".to_string(),
            name: "/tmp/query.rs L:20-".to_string(),
            path: PathBuf::from("/tmp/query.rs"),
        }];

        normalize_read_actions(&mut actions, Path::new("/workspace"));

        assert_eq!(
            actions,
            vec![ParsedCommand::Read {
                cmd: "read".to_string(),
                name: "/tmp/query.rs L:20-".to_string(),
                path: PathBuf::from("/tmp/query.rs"),
            }]
        );
    }

    #[test]
    fn uses_relative_path_over_basename() {
        let mut actions = vec![ParsedCommand::Read {
            cmd: "read".to_string(),
            name: "query.rs L:1-5".to_string(),
            path: PathBuf::from("crates/core/src/query.rs"),
        }];

        normalize_read_actions(&mut actions, Path::new("/workspace"));

        assert_eq!(
            actions,
            vec![ParsedCommand::Read {
                cmd: "read".to_string(),
                name: "crates/core/src/query.rs L:1-5".to_string(),
                path: PathBuf::from("crates/core/src/query.rs"),
            }]
        );
    }

    #[test]
    fn normalizes_absolute_search_path_to_relative() {
        let mut actions = vec![ParsedCommand::Search {
            cmd: "grep pattern /workspace/src/file.rs".to_string(),
            query: Some("pattern".to_string()),
            path: Some("/workspace/src/file.rs".to_string()),
        }];

        normalize_read_actions(&mut actions, Path::new("/workspace"));

        assert_eq!(
            actions,
            vec![ParsedCommand::Search {
                cmd: "grep pattern /workspace/src/file.rs".to_string(),
                query: Some("pattern".to_string()),
                path: Some("src/file.rs".to_string()),
            }]
        );
    }

    #[test]
    fn keeps_absolute_search_path_outside_cwd() {
        let mut actions = vec![ParsedCommand::Search {
            cmd: "grep pattern /tmp/file.rs".to_string(),
            query: Some("pattern".to_string()),
            path: Some("/tmp/file.rs".to_string()),
        }];

        normalize_read_actions(&mut actions, Path::new("/workspace"));

        assert_eq!(
            actions,
            vec![ParsedCommand::Search {
                cmd: "grep pattern /tmp/file.rs".to_string(),
                query: Some("pattern".to_string()),
                path: Some("/tmp/file.rs".to_string()),
            }]
        );
    }

    #[test]
    fn normalizes_absolute_listfiles_path_to_relative() {
        let mut actions = vec![ParsedCommand::ListFiles {
            cmd: "find *.rs /workspace/src".to_string(),
            path: Some("/workspace/src".to_string()),
        }];

        normalize_read_actions(&mut actions, Path::new("/workspace"));

        assert_eq!(
            actions,
            vec![ParsedCommand::ListFiles {
                cmd: "find *.rs /workspace/src".to_string(),
                path: Some("src".to_string()),
            }]
        );
    }

    #[test]
    fn keeps_relative_search_path_unchanged() {
        let mut actions = vec![ParsedCommand::Search {
            cmd: "grep pattern src/file.rs".to_string(),
            query: Some("pattern".to_string()),
            path: Some("src/file.rs".to_string()),
        }];

        normalize_read_actions(&mut actions, Path::new("/workspace"));

        assert_eq!(
            actions,
            vec![ParsedCommand::Search {
                cmd: "grep pattern src/file.rs".to_string(),
                query: Some("pattern".to_string()),
                path: Some("src/file.rs".to_string()),
            }]
        );
    }

    #[test]
    fn relativize_path_equal_to_cwd_shows_folder_name() {
        let mut actions = vec![ParsedCommand::Search {
            cmd: "grep pattern /workspace/my-project".to_string(),
            query: Some("pattern".to_string()),
            path: Some("/workspace/my-project".to_string()),
        }];

        normalize_read_actions(&mut actions, Path::new("/workspace/my-project"));

        assert_eq!(
            actions,
            vec![ParsedCommand::Search {
                cmd: "grep pattern /workspace/my-project".to_string(),
                query: Some("pattern".to_string()),
                path: Some("my-project".to_string()),
            }]
        );
    }

    #[test]
    fn read_path_equal_to_cwd_shows_folder_name() {
        let mut actions = vec![ParsedCommand::Read {
            cmd: "read".to_string(),
            name: "/workspace/my-project/src/main.rs L:1-5".to_string(),
            path: PathBuf::from("/workspace/my-project"),
        }];

        normalize_read_actions(&mut actions, Path::new("/workspace/my-project"));

        assert_eq!(
            actions,
            vec![ParsedCommand::Read {
                cmd: "read".to_string(),
                name: "my-project L:1-5".to_string(),
                path: PathBuf::from("/workspace/my-project"),
            }]
        );
    }

    #[test]
    fn listfiles_path_equal_to_cwd_shows_folder_name() {
        let mut actions = vec![ParsedCommand::ListFiles {
            cmd: "find *.rs /workspace/my-project".to_string(),
            path: Some("/workspace/my-project".to_string()),
        }];

        normalize_read_actions(&mut actions, Path::new("/workspace/my-project"));

        assert_eq!(
            actions,
            vec![ParsedCommand::ListFiles {
                cmd: "find *.rs /workspace/my-project".to_string(),
                path: Some("my-project".to_string()),
            }]
        );
    }
}
