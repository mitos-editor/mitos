//! Theme source discovery and inheritance, independent of editor styles.

use crate::merge_toml_values;
use anyhow::{anyhow, Result};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str,
    sync::{Arc, LazyLock},
};
use toml::{map::Map, Value};

pub static DEFAULT_THEME_DATA: LazyLock<Value> = LazyLock::new(|| {
    let bytes = include_bytes!("../../../runtime/themes/base16_terminal.toml");
    toml::from_str(str::from_utf8(bytes).unwrap()).expect("Failed to parse default theme")
});

pub static BASE16_DEFAULT_THEME_DATA: LazyLock<Value> = LazyLock::new(|| {
    let bytes = include_bytes!("../../../base16_theme.toml");
    toml::from_str(str::from_utf8(bytes).unwrap()).expect("Failed to parse base 16 default theme")
});

/// Ordered runtime/configuration roots for one theme setup.
///
/// Each root's `themes` directory is searched from highest to lowest priority.
/// Clones share paths; sources are read on demand, including inherited themes.
#[derive(Clone, Debug)]
pub struct Resources {
    theme_dirs: Arc<[PathBuf]>,
}

impl Resources {
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self {
            theme_dirs: dirs.into_iter().map(|dir| dir.join("themes")).collect(),
        }
    }

    /// Read and merge theme sources without interpreting palettes or styles.
    /// Built-in names always refer to the embedded defaults.
    pub fn load(&self, name: &str) -> Result<Value> {
        match name {
            "default" => Ok(DEFAULT_THEME_DATA.clone()),
            "base16_default" => Ok(BASE16_DEFAULT_THEME_DATA.clone()),
            _ => self.load_theme(name, &mut HashSet::new()),
        }
    }

    /// Discover names from these same roots, plus built-ins, sorted and deduplicated.
    /// Unreadable/missing directories are ignored, as in theme completion.
    pub fn names(&self) -> Vec<String> {
        let mut names = vec!["default".into(), "base16_default".into()];
        for dir in self.theme_dirs.iter() {
            names.extend(Self::read_names(dir));
        }
        names.sort();
        names.dedup();
        names
    }

    /// Recursively load a theme, merging with any inherited parent themes.
    ///
    /// The paths that have been visited in the inheritance hierarchy are tracked
    /// to detect and avoid cycling.
    ///
    /// It is possible for one file to inherit from another file with the same name
    /// so long as the second file is in a themes directory with lower priority.
    /// However, it is not recommended that users do this as it will make tracing
    /// errors more difficult.
    fn load_theme(&self, name: &str, visited_paths: &mut HashSet<PathBuf>) -> Result<Value> {
        let path = self.path(name, visited_paths)?;

        let theme_toml = self.load_toml(path)?;

        let inherits = theme_toml.get("inherits");

        let theme_toml = if let Some(parent_theme_name) = inherits {
            let parent_theme_name = parent_theme_name.as_str().ok_or_else(|| {
                anyhow!("Expected 'inherits' to be a string: {}", parent_theme_name)
            })?;

            let parent_theme_toml = match parent_theme_name {
                // load default themes's toml from const.
                "default" => DEFAULT_THEME_DATA.clone(),
                "base16_default" => BASE16_DEFAULT_THEME_DATA.clone(),
                _ => self.load_theme(parent_theme_name, visited_paths)?,
            };

            self.merge_themes(parent_theme_toml, theme_toml)
        } else {
            theme_toml
        };

        Ok(theme_toml)
    }

    fn read_names(path: &Path) -> Vec<String> {
        std::fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        let entry = entry.ok()?;
                        let path = entry.path();
                        (path.extension()? == "toml")
                            .then(|| path.file_stem().unwrap().to_string_lossy().into_owned())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    // merge one theme into the parent theme
    fn merge_themes(&self, parent_theme_toml: Value, theme_toml: Value) -> Value {
        let parent_palette = parent_theme_toml.get("palette");
        let palette = theme_toml.get("palette");

        // handle the table separately since it needs a `merge_depth` of 2
        // this would conflict with the rest of the theme merge strategy
        let palette_values = match (parent_palette, palette) {
            (Some(parent_palette), Some(palette)) => {
                merge_toml_values(parent_palette.clone(), palette.clone(), 2)
            }
            (Some(parent_palette), None) => parent_palette.clone(),
            (None, Some(palette)) => palette.clone(),
            (None, None) => Map::new().into(),
        };

        // add the palette correctly as nested table
        let mut palette = Map::new();
        palette.insert(String::from("palette"), palette_values);

        // merge the theme into the parent theme
        let theme = merge_toml_values(parent_theme_toml, theme_toml, 1);
        // merge the before specially handled palette into the theme
        merge_toml_values(theme, palette.into(), 1)
    }

    // Loads the theme data as `toml::Value`
    fn load_toml(&self, path: PathBuf) -> Result<Value> {
        let data = std::fs::read_to_string(path)?;
        let value = toml::from_str(&data)?;

        Ok(value)
    }

    /// Returns the path to the theme with the given name
    ///
    /// Ignores paths already visited and follows directory priority order.
    fn path(&self, name: &str, visited_paths: &mut HashSet<PathBuf>) -> Result<PathBuf> {
        let filename = format!("{}.toml", name);

        let mut cycle_found = false; // track if there was a path, but it was in a cycle
        self.theme_dirs
            .iter()
            .find_map(|dir| {
                let path = dir.join(&filename);
                if !path.exists() {
                    None
                } else if visited_paths.contains(&path) {
                    // Avoiding cycle, continuing to look in lower priority directories
                    cycle_found = true;
                    None
                } else {
                    visited_paths.insert(path.clone());
                    Some(path)
                }
            })
            .ok_or_else(|| {
                if cycle_found {
                    anyhow!("Cycle found in inheriting: {}", name)
                } else {
                    anyhow!("File not found for: {}", name)
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, name: &str, source: &str) {
        std::fs::create_dir_all(root.join("themes")).unwrap();
        std::fs::write(root.join("themes").join(format!("{name}.toml")), source).unwrap();
    }

    #[test]
    fn same_name_inheritance_preserves_palette_merging_and_style_replacement() {
        let high = tempfile::tempdir().unwrap();
        let low = tempfile::tempdir().unwrap();
        write(
            low.path(),
            "custom",
            r##"
            "keyword" = { fg = "accent", bg = "base", modifiers = ["bold"] }
            "string" = "base"
            [palette]
            accent = "#112233"
            base = "#445566"
        "##,
        );
        write(
            high.path(),
            "custom",
            r##"
            inherits = "custom"
            "keyword" = { fg = "accent" }
            [palette]
            accent = "#aabbcc"
        "##,
        );
        let resources = Resources::new(vec![high.path().into(), low.path().into()]);
        let source = resources.load("custom").unwrap();
        assert_eq!(source["palette"]["accent"].as_str(), Some("#aabbcc"));
        assert_eq!(source["palette"]["base"].as_str(), Some("#445566"));
        assert_eq!(source["string"].as_str(), Some("base"));
        assert_eq!(source["keyword"].as_table().unwrap().len(), 1);
        assert_eq!(source["keyword"]["fg"].as_str(), Some("accent"));
        // Visited paths belong to one load, not to the resource instance.
        assert_eq!(resources.load("custom").unwrap(), source);
    }

    #[test]
    fn parents_use_full_priority_order_even_for_a_child_in_a_lower_root() {
        let high = tempfile::tempdir().unwrap();
        let low = tempfile::tempdir().unwrap();
        write(low.path(), "child", "inherits = 'parent'\nchild = 'green'");
        write(low.path(), "parent", "parent = 'red'");
        write(
            high.path(),
            "parent",
            "inherits = 'grandparent'\nparent = 'blue'",
        );
        write(low.path(), "grandparent", "grandparent = 'yellow'");
        let resources = Resources::new(vec![high.path().into(), low.path().into()]);
        let source = resources.load("child").unwrap();
        assert_eq!(source["child"].as_str(), Some("green"));
        assert_eq!(source["parent"].as_str(), Some("blue"));
        assert_eq!(source["grandparent"].as_str(), Some("yellow"));
    }

    #[test]
    fn builtins_are_reserved_but_can_be_inherited() {
        let dir = tempfile::tempdir().unwrap();
        let resources = Resources::new(vec![dir.path().into()]);
        for (name, expected) in [
            ("default", &*DEFAULT_THEME_DATA),
            ("base16_default", &*BASE16_DEFAULT_THEME_DATA),
        ] {
            write(dir.path(), name, "invalid TOML[");
            assert_eq!(&resources.load(name).unwrap(), expected);
            write(
                dir.path(),
                "child",
                &format!("inherits = '{name}'\n'ui.text' = 'red'"),
            );
            let child = resources.load("child").unwrap();
            assert_eq!(child["ui.text"].as_str(), Some("red"));
            assert_eq!(child["ui.selection"], expected["ui.selection"]);
        }
    }

    #[test]
    fn missing_invalid_and_cyclic_sources_keep_errors_without_falling_back() {
        let high = tempfile::tempdir().unwrap();
        let low = tempfile::tempdir().unwrap();
        let resources = Resources::new(vec![high.path().into(), low.path().into()]);
        assert!(resources
            .load("missing")
            .unwrap_err()
            .to_string()
            .contains("File not found for: missing"));
        write(high.path(), "first", "inherits = 'second'");
        write(high.path(), "second", "inherits = 'first'");
        assert!(resources
            .load("first")
            .unwrap_err()
            .to_string()
            .contains("Cycle found in inheriting: first"));
        write(high.path(), "invalid-inherits", "inherits = 42");
        assert!(resources
            .load("invalid-inherits")
            .unwrap_err()
            .to_string()
            .contains("Expected 'inherits' to be a string"));
        write(low.path(), "broken", "keyword = 'blue'");
        write(high.path(), "broken", "invalid TOML[");
        assert!(resources.load("broken").is_err());
        std::fs::remove_file(high.path().join("themes/broken.toml")).unwrap();
        std::fs::create_dir(high.path().join("themes/broken.toml")).unwrap();
        assert!(resources.load("broken").is_err());
        std::fs::remove_dir(high.path().join("themes/broken.toml")).unwrap();
        write(high.path(), "broken", "");
        assert!(resources
            .load("broken")
            .unwrap()
            .as_table()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn discovery_is_sorted_deduplicated_and_isolated_and_reads_stay_lazy() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let a = Resources::new(vec![first.path().into(), first.path().join("missing")]);
        let b = Resources::new(vec![second.path().into()]);
        let clone = a.clone();
        assert_eq!(a.names(), ["base16_default", "default"]);
        write(first.path(), "zebra", "keyword = 'blue'");
        write(first.path(), "default", "keyword = 'red'");
        write(second.path(), "alpha", "keyword = 'green'");
        write(second.path(), "zebra", "keyword = 'yellow'");
        std::fs::write(first.path().join("themes/ignored.txt"), "").unwrap();
        assert_eq!(clone.names(), ["base16_default", "default", "zebra"]);
        assert_eq!(b.names(), ["alpha", "base16_default", "default", "zebra"]);
        let both = Resources::new(vec![first.path().into(), second.path().into()]);
        assert_eq!(both.names(), b.names());
        assert_eq!(a.load("zebra").unwrap()["keyword"].as_str(), Some("blue"));
        assert_eq!(b.load("zebra").unwrap()["keyword"].as_str(), Some("yellow"));
        write(first.path(), "zebra", "keyword = 'red'");
        assert_eq!(
            clone.load("zebra").unwrap()["keyword"].as_str(),
            Some("red")
        );
        std::fs::remove_file(first.path().join("themes/zebra.toml")).unwrap();
        assert!(!clone.names().contains(&"zebra".into()));
    }
}
