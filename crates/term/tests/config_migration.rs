use loader::workspace_trust::{Config as TrustConfig, TrustStatus, WorkspaceTrust};
use std::{fs, process::Command};
use term::config::{Config, ConfigMigration, MigrationScope};

#[path = "test/helpers/isolation.rs"]
mod isolation;

// Exercise explicit migration of each scope while retaining the combined-layer assertions.
fn migrate_both() -> anyhow::Result<(Config, Vec<ConfigMigration>)> {
    let config = Config::load_default().map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let mut notices = Vec::new();
    if loader::config_file().exists() {
        notices.extend(Config::migrate(MigrationScope::User)?);
    }
    if Config::migration_path(MigrationScope::Workspace).is_ok()
        && loader::workspace_config_file().exists()
    {
        notices.extend(Config::migrate(MigrationScope::Workspace)?);
    }
    Ok((config, notices))
}

#[test]
fn migrates_both_layers_with_backups_and_preserves_precedence() -> anyhow::Result<()> {
    let Some(root) =
        isolation::workspace("migrates_both_layers_with_backups_and_preserves_precedence")?
    else {
        return Ok(());
    };
    let user = loader::config_file();
    let project = loader::workspace_config_file();
    let global = "# User settings\n[editor]\nscrolloff = 7\nauto-format = false\n[editor.workspace-trust]\nlevel = 'insecure'\n[keys.normal]\nC-s = ':write'\n";
    let local =
        "# Project settings\n[editor]\nscrolloff = 17\n[editor.lsp]\ndisplay-messages = false\n";
    fs::write(&user, global)?;
    fs::write(&project, local)?;
    let languages = project.with_file_name("languages.toml");
    let language_source = "[[language]]\nname = 'rust'\nauto-format = false\n";
    fs::write(&languages, language_source)?;
    let (config, notices) = migrate_both().unwrap();
    assert_eq!(config.editor.scroll.padding, 17);
    assert!(!config.editor.lsp.messages.enable);
    assert!(!config.editor.format.on_save);
    assert_eq!(notices.len(), 2);
    assert!(notices
        .iter()
        .all(|n| matches!(n, ConfigMigration::Updated { .. })));
    for (path, original) in [(&user, global), (&project, local)] {
        assert_eq!(
            fs::read_to_string(path.with_extension("toml.bak"))?,
            original
        );
        assert!(!fs::read_to_string(path)?.contains("scrolloff"));
    }
    assert!(fs::read_to_string(&user)?.contains("[keys.normal]\nC-s = ':write'"));
    assert!(fs::read_to_string(&project)?.contains("# Project settings"));
    assert_eq!(fs::read_to_string(languages)?, language_source);
    assert_eq!(
        WorkspaceTrust::new(TrustConfig::default()).status(&root.join("workspace")),
        TrustStatus::Untrusted
    );
    assert!(migrate_both().unwrap().1.is_empty());
    assert_eq!(fs::read_dir(user.parent().unwrap())?.count(), 2);
    assert_eq!(fs::read_dir(project.parent().unwrap())?.count(), 3);
    Ok(())
}

#[test]
fn untrusted_stale_and_excluded_projects_are_not_rewritten() -> anyhow::Result<()> {
    let Some(root) =
        isolation::workspace("untrusted_stale_and_excluded_projects_are_not_rewritten")?
    else {
        return Ok(());
    };
    let workspace = root.join("workspace");
    let project = loader::workspace_config_file();
    let global = "[editor.scroll]\npadding = 7\n[editor.workspace-trust]\nlevel = 'none'\n";
    let local = "[editor]\nscrolloff = 17\n";
    fs::write(loader::config_file(), global)?;
    fs::write(&project, local)?;
    let trust = WorkspaceTrust::new(TrustConfig::default());
    // No grant: the project config is not interpreted or rewritten.
    let (config, notices) = migrate_both().unwrap();
    assert_eq!(config.editor.scroll.padding, 7);
    assert!(notices.is_empty());
    trust.trust(&workspace);
    fs::write(
        project.with_file_name("languages.toml"),
        "# Changed after approval\n",
    )?;
    assert_eq!(
        WorkspaceTrust::new(TrustConfig::default()).status(&workspace),
        TrustStatus::Stale
    );
    assert!(migrate_both().unwrap().1.is_empty());
    trust.exclude(&workspace);
    assert!(migrate_both().unwrap().1.is_empty());
    assert_eq!(fs::read_to_string(project)?, local);
    assert_eq!(fs::read_dir(workspace.join(".mitos"))?.count(), 2);
    Ok(())
}

#[test]
fn preserves_explicit_trust_across_migration_and_reload() -> anyhow::Result<()> {
    let Some(root) = isolation::workspace("preserves_explicit_trust_across_migration_and_reload")?
    else {
        return Ok(());
    };
    let workspace = root.join("workspace");
    fs::write(
        loader::config_file(),
        "[editor]\nscrolloff = 7\n[editor.workspace-trust]\nlevel = 'none'\n",
    )?;
    fs::write(
        loader::workspace_config_file(),
        "[editor]\nscrolloff = 17\n",
    )?;
    WorkspaceTrust::new(TrustConfig::default()).trust(&workspace);
    let (config, notices) = migrate_both().unwrap();
    assert_eq!(config.editor.scroll.padding, 17);
    assert!(notices
        .iter()
        .all(|n| matches!(n, ConfigMigration::Updated { .. })));
    assert_eq!(
        WorkspaceTrust::new(TrustConfig::default()).status(&workspace),
        TrustStatus::Trusted
    );
    let (config, notices) = migrate_both().unwrap();
    assert_eq!(config.editor.scroll.padding, 17);
    assert!(notices.is_empty());
    Ok(())
}

#[test]
fn migration_cannot_approve_concurrent_configuration_changes() -> anyhow::Result<()> {
    let Some(root) =
        isolation::workspace("migration_cannot_approve_concurrent_configuration_changes")?
    else {
        return Ok(());
    };
    let workspace = root.join("workspace");
    let project = loader::workspace_config_file();
    let languages = project.with_file_name("languages.toml");
    for tamper_with_project in [false, true] {
        fs::write(&project, "[editor]\nscrolloff = 17\n")?;
        fs::write(&languages, "# Approved language config\n")?;
        let trust = WorkspaceTrust::new(TrustConfig::default());
        trust.trust(&workspace);
        let written = loader::config::migration::file(&project, |_| Ok(()))?.unwrap();
        if tamper_with_project {
            fs::write(&project, "[editor.scroll]\npadding = 999\n")?;
        } else {
            fs::write(&languages, "# Changed concurrently\n")?;
        }
        assert!(!trust.record_config_migration(&workspace, &written));
        assert_eq!(
            WorkspaceTrust::new(TrustConfig::default()).status(&workspace),
            TrustStatus::Stale
        );
    }
    Ok(())
}

#[test]
fn validates_all_loaded_layers_before_writing() -> anyhow::Result<()> {
    let Some(_) = isolation::workspace("validates_all_loaded_layers_before_writing")? else {
        return Ok(());
    };
    let user = loader::config_file();
    let project = loader::workspace_config_file();
    let original = "[editor]\nscrolloff = 7\n[editor.workspace-trust]\nlevel = 'insecure'\n";
    fs::write(&user, original)?;
    fs::write(&project, "[editor]\nunknown-setting = true\n")?;
    assert!(migrate_both().is_err());
    assert_eq!(fs::read_to_string(&user)?, original);
    assert!(!user.with_extension("toml.bak").exists());
    assert!(!project.with_extension("toml.bak").exists());
    fs::remove_file(&project)?;
    fs::write(
        &user,
        "[editor]\nauto-format = true\nformat.on-save = false\n",
    )?;
    assert!(migrate_both().is_err());
    assert!(!user.with_extension("toml.bak").exists());
    Ok(())
}

#[test]
fn read_only_commands_leave_both_configs_untouched() -> anyhow::Result<()> {
    let Some(_) = isolation::workspace("read_only_commands_leave_both_configs_untouched")? else {
        return Ok(());
    };
    let user = loader::config_file();
    let project = loader::workspace_config_file();
    let global = "[editor]\nscrolloff = 7\n[editor.workspace-trust]\nlevel = 'insecure'\n";
    let local = "[editor]\nscrolloff = 17\n";
    fs::write(&user, global)?;
    fs::write(&project, local)?;
    assert_eq!(Config::load_default().unwrap().editor.scroll.padding, 17);
    for args in [
        vec!["--health", "clipboard"],
        vec!["--help"],
        vec!["--version"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ms"))
            .args(args)
            .arg("-c")
            .arg(&user)
            .output()?;
        assert!(output.status.success(), "{output:?}");
        assert_eq!(fs::read_to_string(&user)?, global);
        assert_eq!(fs::read_to_string(&project)?, local);
        assert!(!user.with_extension("toml.bak").exists());
        assert!(!project.with_extension("toml.bak").exists());
    }
    Ok(())
}

#[test]
fn export_migrates_only_the_requested_file_without_writing() -> anyhow::Result<()> {
    let Some(_) = isolation::workspace("export_migrates_only_the_requested_file_without_writing")?
    else {
        return Ok(());
    };
    let user = loader::config_file();
    let project = loader::workspace_config_file();
    let source = "# Managed settings\n[editor]\nscrolloff = 7\n";
    fs::write(&user, source)?;
    // Export must not read or validate the workspace configuration.
    fs::write(&project, "invalid = [")?;
    let export = || {
        Command::new(env!("CARGO_BIN_EXE_ms"))
            .arg("--migrate-config")
            .arg(&user)
            .arg("--stdout")
            .output()
    };
    let output = export()?;
    assert!(output.status.success(), "{output:?}");
    let migrated = String::from_utf8(output.stdout)?;
    assert_eq!(
        migrated,
        loader::config::migration::source(source)?.unwrap()
    );
    assert_eq!(fs::read_to_string(&user)?, source);
    assert_eq!(fs::read_dir(user.parent().unwrap())?.count(), 1);
    assert_eq!(fs::read_to_string(&project)?, "invalid = [");
    fs::write(&user, &migrated)?;
    assert_eq!(export()?.stdout, migrated.as_bytes());
    for args in [
        vec!["--stdout"],
        vec!["--migrate-config"],
        vec!["--migrate-config", "--stdout"],
    ] {
        assert!(!Command::new(env!("CARGO_BIN_EXE_ms"))
            .args(args)
            .output()?
            .status
            .success());
    }
    fs::write(&user, "invalid = [")?;
    assert!(!export()?.status.success());
    Ok(())
}

#[cfg(unix)]
#[test]
fn linked_configs_load_and_export_without_modifying_the_target() -> anyhow::Result<()> {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let Some(root) =
        isolation::workspace("linked_configs_load_and_export_without_modifying_the_target")?
    else {
        return Ok(());
    };
    let user = loader::config_file();
    let store = root.join("managed");
    fs::create_dir(&store)?;
    let target = store.join("config.toml");
    let source = "# Managed settings\n[editor]\nscrolloff = 9\n";
    fs::write(&target, source)?;
    symlink(&target, &user)?;
    for readonly in [false, true] {
        if readonly {
            fs::set_permissions(&target, fs::Permissions::from_mode(0o444))?;
            fs::set_permissions(&store, fs::Permissions::from_mode(0o555))?;
        }
        let loaded = Config::load_default();
        let output = Command::new(env!("CARGO_BIN_EXE_ms"))
            .arg("--migrate-config")
            .arg(&user)
            .arg("--stdout")
            .output();
        fs::set_permissions(&store, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644))?;
        assert_eq!(loaded.unwrap().editor.scroll.padding, 9);
        let output = output?;
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty());
        assert_eq!(
            output.stdout,
            loader::config::migration::source(source)?
                .unwrap()
                .as_bytes()
        );
        assert_eq!(fs::read_to_string(&target)?, source);
        assert!(fs::symlink_metadata(&user)?.is_symlink());
        assert_eq!(fs::read_dir(&store)?.count(), 1);
    }
    Ok(())
}

#[test]
fn explicit_migration_is_scoped_and_requires_workspace_trust() -> anyhow::Result<()> {
    let Some(root) =
        isolation::workspace("explicit_migration_is_scoped_and_requires_workspace_trust")?
    else {
        return Ok(());
    };
    let user = loader::config_file();
    let project = loader::workspace_config_file();
    let global = "[editor]\nscrolloff = 7\n[editor.workspace-trust]\nlevel = 'none'\n";
    let local = "[editor]\nscrolloff = 17\n";
    fs::write(&user, global)?;
    fs::write(&project, local)?;
    assert!(Config::migrate(MigrationScope::Workspace).is_err());
    Config::migrate(MigrationScope::User)?;
    assert_eq!(fs::read_to_string(&project)?, local);
    assert!(!project.with_extension("toml.bak").exists());
    let migrated_user = fs::read_to_string(&user)?;
    WorkspaceTrust::new(TrustConfig::default()).trust(&root.join("workspace"));
    Config::migrate(MigrationScope::Workspace)?;
    assert_eq!(fs::read_to_string(&user)?, migrated_user);
    assert!(!user.with_extension("toml.bak.1").exists());
    assert_eq!(
        fs::read_to_string(project.with_extension("toml.bak"))?,
        local
    );
    Ok(())
}

#[test]
fn trusted_project_migrates_without_creating_a_user_config() -> anyhow::Result<()> {
    let Some(root) =
        isolation::workspace("trusted_project_migrates_without_creating_a_user_config")?
    else {
        return Ok(());
    };
    fs::write(
        loader::workspace_config_file(),
        "[editor]\nscrolloff = 17\n",
    )?;
    WorkspaceTrust::new(TrustConfig::default()).trust(&root.join("workspace"));
    let (config, notices) = migrate_both().unwrap();
    assert_eq!(config.editor.scroll.padding, 17);
    assert_eq!(notices.len(), 1);
    assert!(!loader::config_file().exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn project_write_failure_keeps_loaded_settings() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let Some(_) = isolation::workspace("project_write_failure_keeps_loaded_settings")? else {
        return Ok(());
    };
    if unsafe { libc::geteuid() } == 0 {
        return Ok(());
    }
    let project = loader::workspace_config_file();
    let original = "[editor]\nscrolloff = 17\n";
    fs::write(
        loader::config_file(),
        "[editor.workspace-trust]\nlevel = 'insecure'\n",
    )?;
    fs::write(&project, original)?;
    let parent = project.parent().unwrap();
    let permissions = fs::metadata(parent)?.permissions();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o555))?;
    let result = migrate_both();
    fs::set_permissions(parent, permissions)?;
    let (config, notices) = result.unwrap();
    assert_eq!(config.editor.scroll.padding, 17);
    assert!(matches!(notices.as_slice(), [ConfigMigration::Warning(_)]));
    assert_eq!(fs::read_to_string(project)?, original);
    Ok(())
}

#[cfg(unix)]
#[test]
fn project_symlink_migration_preserves_the_link_and_trust() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;
    let Some(root) =
        isolation::workspace("project_symlink_migration_preserves_the_link_and_trust")?
    else {
        return Ok(());
    };
    let workspace = root.join("workspace");
    let project = loader::workspace_config_file();
    let target = root.join("project-dotfile.toml");
    let original = "# Linked config\n[editor]\nscrolloff = 17\n";
    fs::write(loader::config_file(), "")?;
    fs::write(&target, original)?;
    symlink(&target, &project)?;
    WorkspaceTrust::new(TrustConfig::default()).trust(&workspace);
    let (config, notices) = migrate_both().unwrap();
    assert_eq!(config.editor.scroll.padding, 17);
    assert!(
        matches!(notices.as_slice(), [ConfigMigration::Updated { .. }]),
        "{notices:?}"
    );
    assert!(fs::symlink_metadata(project)?.is_symlink());
    assert_eq!(
        fs::read_to_string(target.with_extension("toml.bak"))?,
        original
    );
    assert_eq!(
        WorkspaceTrust::new(TrustConfig::default()).status(&workspace),
        TrustStatus::Trusted
    );
    Ok(())
}

#[test]
fn project_migration_retains_required_fields_inherited_from_the_user() -> anyhow::Result<()> {
    let Some(_) =
        isolation::workspace("project_migration_retains_required_fields_inherited_from_the_user")?
    else {
        return Ok(());
    };
    let project = loader::workspace_config_file();
    fs::write(
        loader::config_file(),
        "[editor.workspace-trust]\nlevel = 'insecure'\n[terminal.launcher]\ncommand = 'tmux'\n",
    )?;
    let original = "[editor.terminal]\nargs = ['split-window']\n";
    fs::write(&project, original)?;
    let (config, notices) = migrate_both().unwrap();
    let launcher = config.terminal.launcher.unwrap();
    assert_eq!(launcher.command, "tmux");
    assert_eq!(launcher.args, ["split-window"]);
    assert!(
        matches!(notices.as_slice(), [ConfigMigration::Updated { .. }]),
        "{notices:?}"
    );
    assert_eq!(
        fs::read_to_string(project.with_extension("toml.bak"))?,
        original
    );
    assert!(!fs::read_to_string(project)?.contains("editor.terminal"));
    Ok(())
}
