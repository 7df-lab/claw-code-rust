use dirs::home_dir;
use std::path::PathBuf;

fn strip_unc_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.display().to_string();
        if let Some(stripped) = s.strip_prefix("\\\\?\\") {
            return PathBuf::from(stripped);
        }
    }
    path
}

/// Returns the path to the Devo configuration directory, which can be
/// specified by the `DEVO_HOME` environment variable. If not set, defaults to
/// `~/.devo`.
///
/// - If `DEVO_HOME` is set, the value must exist and be a directory. The
///   value will be canonicalized and this function will Err otherwise.
/// - If `DEVO_HOME` is not set, this function does not verify that the
///   directory exists.
pub fn find_devo_home() -> std::io::Result<PathBuf> {
    let devo_home_env = std::env::var("DEVO_HOME")
        .ok()
        .filter(|val| !val.is_empty());
    find_devo_home_from_env(devo_home_env.as_deref())
}

fn find_devo_home_from_env(devo_home_env: Option<&str>) -> std::io::Result<PathBuf> {
    // Honor the `DEVO_HOME` environment variable when it is set to allow users
    // (and tests) to override the default location.
    match devo_home_env {
        Some(val) => {
            let path = PathBuf::from(val);
            let metadata = std::fs::metadata(&path).map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("DEVO_HOME points to {val:?}, but that path does not exist"),
                ),
                _ => std::io::Error::new(
                    err.kind(),
                    format!("failed to read DEVO_HOME {val:?}: {err}"),
                ),
            })?;

            if !metadata.is_dir() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("DEVO_HOME points to {val:?}, but that path is not a directory"),
                ))
            } else {
                path.canonicalize().map(strip_unc_prefix).map_err(|err| {
                    std::io::Error::new(
                        err.kind(),
                        format!("failed to canonicalize DEVO_HOME {val:?}: {err}"),
                    )
                })
            }
        }
        None => {
            let mut p = home_dir().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Could not find home directory",
                )
            })?;
            p.push(".devo");
            Ok(p)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::find_devo_home_from_env;
    use dirs::home_dir;
    use std::fs;
    use std::io::ErrorKind;
    use tempfile::TempDir;

    #[test]
    fn find_devo_home_env_missing_path_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let missing = temp_home.path().join("missing-devo-home");
        let missing_str = missing
            .to_str()
            .expect("missing devo home path should be valid utf-8");

        let err = find_devo_home_from_env(Some(missing_str)).expect_err("missing DEVO_HOME");
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(
            err.to_string().contains("DEVO_HOME"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_devo_home_env_file_path_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let file_path = temp_home.path().join("devo-home.txt");
        fs::write(&file_path, "not a directory").expect("write temp file");
        let file_str = file_path
            .to_str()
            .expect("file devo home path should be valid utf-8");

        let err = find_devo_home_from_env(Some(file_str)).expect_err("file DEVO_HOME");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("not a directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_devo_home_env_valid_directory_canonicalizes() {
        let temp_home = TempDir::new().expect("temp home");
        let temp_str = temp_home
            .path()
            .to_str()
            .expect("temp devo home path should be valid utf-8");

        let resolved = find_devo_home_from_env(Some(temp_str)).expect("valid DEVO_HOME");
        let expected = super::strip_unc_prefix(
            temp_home
                .path()
                .canonicalize()
                .expect("canonicalize temp home"),
        );
        assert_eq!(resolved, expected);
    }

    #[test]
    fn find_devo_home_without_env_uses_default_home_dir() {
        let resolved = find_devo_home_from_env(/*devo_home_env*/ None).expect("default DEVO_HOME");
        let mut expected = home_dir().expect("home dir");
        expected.push(".devo");
        assert_eq!(resolved, expected);
    }
}
