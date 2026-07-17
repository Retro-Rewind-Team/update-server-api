//! The update manifest: the single source of truth that the legacy text files
//! are rendered from.
//!
//! The legacy formats are whitespace-separated with one record per line, so no
//! field may contain whitespace.

use serde::{Deserialize, Serialize};

/// Legacy field describing the folder the zip should have been extracted to. Unused
/// but remains in order to not break existing client parsers.
const DESCRIPTION: &str = "Assets";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub versions: Vec<VersionEntry>,
}

/// One release. A release ships zero or more update zips and deletes zero or
/// more files.
///
/// Both are plural because the real data needs it: 4.0.0 ships two zips
/// (`1000.zip` and `1000Music.zip`), and 3.7.1 and 4.0.1 delete files without
/// shipping a zip at all, so they appear in the delete list but never in the
/// version list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionEntry {
    pub version: String,
    /// Download URLs of the update zips. The install path the updaters use is
    /// derived from each URL's basename.
    #[serde(default)]
    pub zips: Vec<String>,
    #[serde(default)]
    pub deletes: Vec<String>,
    /// Full install download, if one was cut at this version. The newest one in
    /// the manifest is what `RetroRewindInstall.txt` serves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_download: Option<String>,
}

/// Where the updaters drop a zip before extracting it: the filename from the
/// URL, at the install root.
fn install_path(url: &str) -> String {
    format!("/{}", basename(url))
}

fn basename(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

/// The rendered text files, cached so requests don't re-render.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub version_txt: String,
    pub delete_txt: String,
    pub install_txt: Option<String>,
}

impl Manifest {
    /// Sort key for a dotted version. Also serves as the version syntax check.
    pub fn version_key(version: &str) -> Result<Vec<u64>, String> {
        if version.is_empty() {
            return Err("version must not be empty".to_owned());
        }
        version
            .split('.')
            .map(|part| {
                part.parse::<u64>()
                    .map_err(|_| format!("version {version:?} must be dotted numbers"))
            })
            .collect()
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut previous: Option<(&str, Vec<u64>)> = None;

        for entry in &self.versions {
            let key = Self::version_key(&entry.version)?;

            if let Some((previous_version, previous_key)) = &previous
                && *previous_key >= key
            {
                return Err(format!(
                    "versions must be strictly ascending, but {:?} follows {previous_version:?}",
                    entry.version
                ));
            }
            previous = Some((&entry.version, key));

            for url in &entry.zips {
                check_field(&entry.version, "zip url", url)?;
                if basename(url).is_empty() {
                    return Err(format!(
                        "{}: zip url must end in a filename, got {url:?}",
                        entry.version
                    ));
                }
            }
            for path in &entry.deletes {
                check_field(&entry.version, "delete path", path)?;
            }
            if let Some(full_download) = &entry.full_download {
                check_field(&entry.version, "full download url", full_download)?;
            }
        }

        Ok(())
    }

    pub fn render(&self) -> Rendered {
        Rendered {
            version_txt: self.render_version_txt(),
            delete_txt: self.render_delete_txt(),
            install_txt: self.render_install_txt(),
        }
    }

    /// `<version> <url> <path> <description>`, one line per zip
    fn render_version_txt(&self) -> String {
        let lines = self.versions.iter().flat_map(|entry| {
            entry.zips.iter().map(move |url| {
                format!(
                    "{} {url} {} {DESCRIPTION}",
                    entry.version,
                    install_path(url)
                )
            })
        });
        join_lines(lines)
    }

    /// `<version> <path>`, one line per deleted file.
    fn render_delete_txt(&self) -> String {
        let lines = self.versions.iter().flat_map(|entry| {
            entry
                .deletes
                .iter()
                .map(move |path| format!("{} {}", entry.version, path))
        });
        join_lines(lines)
    }

    /// The newest full download in the manifest.
    fn render_install_txt(&self) -> Option<String> {
        self.versions
            .iter()
            .rev()
            .find_map(|entry| entry.full_download.clone())
    }
}

/// The legacy files have no trailing newline; matching that keeps the output
/// byte-identical to what the static filestore served.
fn join_lines(lines: impl Iterator<Item = String>) -> String {
    lines.collect::<Vec<_>>().join("\n")
}

fn check_field(version: &str, kind: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{version}: {kind} must not be empty"));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(format!(
            "{version}: {kind} must not contain whitespace, got {value:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped manifest must stay loadable and renderable by the code that
    /// serves it.
    #[test]
    fn shipped_manifest_is_valid() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(root.join("manifest.json")).unwrap();

        let manifest: Manifest = serde_json::from_str(&text).unwrap();
        manifest.validate().unwrap();
        assert!(manifest.render().install_txt.is_some());
    }

    fn entry(version: &str) -> VersionEntry {
        VersionEntry {
            version: version.to_owned(),
            zips: vec!["https://cdn.example/a.zip".to_owned()],
            deletes: vec!["/a.zip".to_owned()],
            full_download: None,
        }
    }

    #[test]
    fn version_key_orders_numerically_not_lexically() {
        // "6.9.0" sorts after "6.10.0" as a string, which would corrupt the
        // ordering the updaters depend on.
        assert!(Manifest::version_key("6.9.0").unwrap() < Manifest::version_key("6.10.0").unwrap());
    }

    #[test]
    fn version_key_rejects_non_numeric() {
        assert!(Manifest::version_key("6.x.0").is_err());
        assert!(Manifest::version_key("").is_err());
    }

    #[test]
    fn validate_rejects_out_of_order_versions() {
        let manifest = Manifest {
            versions: vec![entry("1.1.0"), entry("1.0.0")],
        };
        assert!(manifest.validate().unwrap_err().contains("ascending"));
    }

    #[test]
    fn validate_rejects_duplicate_versions() {
        let manifest = Manifest {
            versions: vec![entry("1.0.0"), entry("1.0.0")],
        };
        assert!(manifest.validate().unwrap_err().contains("ascending"));
    }

    #[test]
    fn validate_rejects_whitespace_that_would_corrupt_the_line_format() {
        let mut manifest = Manifest {
            versions: vec![entry("1.0.0")],
        };
        manifest.versions[0].zips[0] = "https://cdn.example/two words.zip".to_owned();
        assert!(manifest.validate().unwrap_err().contains("whitespace"));
    }

    #[test]
    fn validate_rejects_a_url_with_no_filename() {
        let mut manifest = Manifest {
            versions: vec![entry("1.0.0")],
        };
        manifest.versions[0].zips[0] = "https://cdn.example/".to_owned();
        assert!(manifest.validate().unwrap_err().contains("filename"));
    }

    /// The updaters parse four whitespace-separated columns, so the install
    /// path and description still have to be emitted even though the manifest
    /// no longer stores them.
    #[test]
    fn renders_install_path_and_description_from_the_url() {
        let manifest = Manifest {
            versions: vec![VersionEntry {
                version: "6.6.0".to_owned(),
                zips: vec!["https://cdn.update.rwfc.net/RetroRewind/zip/6.6.zip".to_owned()],
                deletes: vec![],
                full_download: None,
            }],
        };
        assert_eq!(
            manifest.render().version_txt,
            "6.6.0 https://cdn.update.rwfc.net/RetroRewind/zip/6.6.zip /6.6.zip Assets"
        );
    }

    #[test]
    fn renders_multiple_zips_for_one_version() {
        let manifest = Manifest {
            versions: vec![VersionEntry {
                version: "4.0.0".to_owned(),
                zips: vec![
                    "https://cdn.example/1000.zip".to_owned(),
                    "https://cdn.example/1000Music.zip".to_owned(),
                ],
                deletes: vec![],
                full_download: None,
            }],
        };
        assert_eq!(
            manifest.render().version_txt,
            "4.0.0 https://cdn.example/1000.zip /1000.zip Assets\n\
             4.0.0 https://cdn.example/1000Music.zip /1000Music.zip Assets"
        );
    }

    #[test]
    fn renders_delete_only_version_without_a_version_line() {
        let manifest = Manifest {
            versions: vec![VersionEntry {
                version: "3.7.1".to_owned(),
                zips: vec![],
                deletes: vec!["/0021.zip".to_owned()],
                full_download: None,
            }],
        };
        let rendered = manifest.render();
        assert_eq!(rendered.version_txt, "");
        assert_eq!(rendered.delete_txt, "3.7.1 /0021.zip");
    }

    #[test]
    fn install_txt_uses_the_newest_full_download() {
        let mut manifest = Manifest {
            versions: vec![entry("1.0.0"), entry("1.1.0"), entry("1.2.0")],
        };
        manifest.versions[0].full_download = Some("https://cdn.example/old.zip".to_owned());
        manifest.versions[1].full_download = Some("https://cdn.example/new.zip".to_owned());
        assert_eq!(
            manifest.render().install_txt.as_deref(),
            Some("https://cdn.example/new.zip")
        );
    }

    #[test]
    fn install_txt_is_absent_when_no_full_download_exists() {
        let manifest = Manifest {
            versions: vec![entry("1.0.0")],
        };
        assert_eq!(manifest.render().install_txt, None);
    }
}
