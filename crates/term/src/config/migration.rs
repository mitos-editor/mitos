//! Trusted filesystem loading and explicitly requested configuration migration.
use super::{Config, ConfigLoadError};
use loader::config::migration;
use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationScope {
    User,
    Workspace,
}

/// Notices from explicitly requested configuration migration.
#[derive(Debug)]
pub enum ConfigMigration {
    Updated {
        path: std::path::PathBuf,
        backup: std::path::PathBuf,
    },
    Warning(String),
}

fn migrate_file(
    path: &std::path::Path,
    global: &str,
    local: Option<&str>,
    is_user: bool,
) -> anyhow::Result<Option<migration::FileMigration>> {
    migration::file(path, |source| {
        // Project tables can inherit required fields from the user layer.
        // Validate the candidate in the same context in which it was loaded.
        let global = if is_user { source } else { global }.to_owned();
        let local = if is_user { local } else { Some(source) }
            .map(str::to_owned)
            .ok_or_else(ConfigLoadError::default);
        Config::load(Ok(&global), local)
            .map(|_| ())
            .map_err(|err| anyhow::anyhow!(err.to_string()))
    })
}

impl Config {
    /// Load user and trusted workspace configuration without rewriting files.
    pub fn load_default() -> Result<Config, ConfigLoadError> {
        Self::load_default_impl(None).map(|(config, _)| config)
    }

    /// Persist known renames in the selected configuration scope.
    /// Write failures become notices while loaded legacy settings remain usable.
    pub fn migrate(scope: MigrationScope) -> anyhow::Result<Vec<ConfigMigration>> {
        let path = Self::migration_path(scope)?;
        fs::read_to_string(&path)?;
        Self::load_default_impl(Some(scope))
            .map(|(_, notices)| notices)
            .map_err(|err| anyhow::anyhow!(err.to_string()))
    }

    /// Check the loaded configuration and trust policy before offering a rewrite.
    pub fn migration_path(scope: MigrationScope) -> anyhow::Result<std::path::PathBuf> {
        let config = Self::load_default().map_err(|err| anyhow::anyhow!(err.to_string()))?;
        match scope {
            MigrationScope::User => Ok(loader::config_file()),
            MigrationScope::Workspace => {
                use workspace::trust::{TrustQuery, WorkspaceTrust};
                let trust = WorkspaceTrust::new(
                    (&config.editor.workspace_trust).into(),
                    loader::workspace_trust_dir(),
                );
                anyhow::ensure!(
                    trust.query_current(TrustQuery::LocalConfig).is_trusted(),
                    "Workspace configuration is not trusted; review and trust it before migrating"
                );
                Ok(loader::workspace_config_file())
            }
        }
    }

    fn load_default_impl(
        scope: Option<MigrationScope>,
    ) -> Result<(Config, Vec<ConfigMigration>), ConfigLoadError> {
        use workspace::trust::{TrustQuery, TrustStatus, WorkspaceTrust};
        let user_path = loader::config_file();
        let (global_config, user_exists) = match fs::read_to_string(&user_path) {
            Ok(source) => (source, true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
            Err(err) => return Err(ConfigLoadError::Error(err)),
        };
        let global_parsed = Config::load(Ok(&global_config), Err(ConfigLoadError::default()))?;
        let trust_config = (&global_parsed.editor.workspace_trust).into();
        let trust = WorkspaceTrust::new(trust_config, loader::workspace_trust_dir());
        let workspace = workspace::find_workspace().0;
        let project_path = workspace.join(".mitos/config.toml");
        let mut paths = Vec::new();
        if user_exists {
            paths.push((user_path, true));
        }

        let mut local_source = None;
        // Check user-scope trust before even reading the project config.
        let config = if trust
            .query(&workspace, TrustQuery::LocalConfig)
            .is_trusted()
        {
            let local = fs::read_to_string(&project_path).map_err(ConfigLoadError::Error);
            if let Ok(source) = &local {
                local_source = Some(source.clone());
                paths.push((project_path, false));
            }
            let mut merged = Config::load(Ok(&global_config), local)?;
            // A project cannot change the policy used to decide whether to load it.
            merged.editor.workspace_trust = global_parsed.editor.workspace_trust;
            merged
        } else {
            global_parsed
        };
        let mut notices = Vec::new();
        if let Some(scope) = scope {
            // All loaded layers must validate before either file is rewritten.
            for (path, is_user) in paths {
                if is_user != (scope == MigrationScope::User) {
                    continue;
                }
                match migrate_file(&path, &global_config, local_source.as_deref(), is_user) {
                    Ok(Some(written)) => {
                        // Never create a grant for implicit trust. Preserve an existing
                        // grant only when the exact migration accounts for the changes.
                        if trust.status(&workspace) == TrustStatus::Trusted
                            && !trust.record_config_migration(
                                &workspace,
                                &workspace::trust::ConfigMigration {
                                    target: &written.target,
                                    backup: &written.backup,
                                    original: &written.original,
                                    updated: &written.updated,
                                },
                            )
                            && WorkspaceTrust::new(
                                (&config.editor.workspace_trust).into(),
                                loader::workspace_trust_dir(),
                            )
                            .status(&workspace)
                                != TrustStatus::Trusted
                        {
                            notices.push(ConfigMigration::Warning(
                                "Workspace trust could not be preserved; review the changes and trust the workspace again".into()
                            ));
                        }
                        log::info!(
                            "Updated {}; backup: {}",
                            path.display(),
                            written.backup.display()
                        );
                        notices.push(ConfigMigration::Updated {
                            path,
                            backup: written.backup,
                        });
                    }
                    Ok(None) => {}
                    Err(err) => {
                        let message = format!(
                            "Could not update {}; using loaded settings: {err:#}. For managed or read-only files, export with ms --migrate-config <file> --stdout and update the configuration source",
                            path.display()
                        );
                        log::warn!("{message}");
                        notices.push(ConfigMigration::Warning(message));
                    }
                }
            }
        }
        Ok((config, notices))
    }
}
