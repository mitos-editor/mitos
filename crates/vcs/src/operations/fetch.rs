//! Native fetching for ordinary branch-tracking remotes.
//! Unsupported Git porcelain settings fall back before any network or ref writes.
use super::*;
use gix::bstr::{BString, ByteSlice};
use std::{io::Write, sync::atomic::AtomicBool};

pub(super) struct Outcome {
    pub target: Option<history::RevisionId>,
}

pub(super) fn execute(
    repository: &GitRepository,
    name: &str,
    branch: Option<&str>,
) -> Result<Option<Outcome>> {
    // Transport and helper errors can contain credentials. Never expose their source chain.
    execute_inner(repository, name, branch).map_err(|_| anyhow::anyhow!(
        "Fetch failed. Check repository state and authentication in your terminal; remote output is hidden because it can contain credentials"
    ))
}

fn execute_inner(
    repository: &GitRepository,
    name: &str,
    branch: Option<&str>,
) -> Result<Option<Outcome>> {
    let mut repo = repository.open(true)?;
    {
        let mut config = repo.config_snapshot_mut();
        config.append_config(
            [
                "gitoxide.credentials.terminalPrompt=false"
                    .as_bytes()
                    .as_bstr(),
                "gitoxide.credentials.helperStderr=false"
                    .as_bytes()
                    .as_bstr(),
            ],
            gix::config::Source::Api,
        )?;
        config.commit()?;
    }
    repo.committer_or_set_generic_fallback()?;
    let config = repo.config_snapshot();
    // gix currently has no pruning or partial-clone fetch implementation.
    for key in [
        "fetch.prune".to_owned(),
        "fetch.pruneTags".into(),
        "fetch.fsckObjects".into(),
        "transfer.fsckObjects".into(),
        "fetch.writeCommitGraph".into(),
        format!("remote.{name}.prune"),
        format!("remote.{name}.pruneTags"),
        format!("remote.{name}.mirror"),
        format!("remote.{name}.promisor"),
    ] {
        if config.plumbing().boolean(key.as_str())?.unwrap_or(false) {
            return Ok(None);
        }
    }
    if [
        "extensions.partialClone".to_owned(),
        format!("remote.{name}.partialCloneFilter"),
        format!("remote.{name}.uploadpack"),
    ]
    .iter()
    .any(|key| config.string(key.as_str()).is_some())
    {
        return Ok(None);
    }
    let follow_head = config.string(format!("remote.{name}.followRemoteHEAD").as_str());
    // Git's default `create` preserves an existing HEAD. Modes which
    // replace it or emit transition warnings remain with Git before any writes.
    if follow_head.as_ref().is_some_and(|value| {
        !value.eq_ignore_ascii_case(b"create") && !value.eq_ignore_ascii_case(b"never")
    }) {
        return Ok(None);
    }
    let update_head = branch.is_none()
        && !follow_head
            .as_ref()
            .is_some_and(|value| value.eq_ignore_ascii_case(b"never"));
    let mut upstream = repo.find_remote(name.as_bytes().as_bstr())?;
    let mut url = upstream
        .url(gix::remote::Direction::Fetch)
        .context("Remote has no fetch URL")?
        .clone();
    // gix's SSH transport forwards helper stderr directly. Keep private remote
    // output and configured askpass behavior with Git until these are controllable.
    if matches!(url.scheme, gix::url::Scheme::Ext | gix::url::Scheme::Ssh)
        || config.string("core.askPass").is_some()
        || std::env::var_os("GIT_ASKPASS").is_some()
        || std::env::var_os("SSH_ASKPASS").is_some()
    {
        return Ok(None);
    }
    if url.scheme == gix::url::Scheme::File {
        let path = gix::path::from_bstr(&url.path)?;
        if !path.is_absolute() {
            url.path = gix::path::into_bstr(repository.root.join(path))?.into_owned();
            upstream = upstream.with_url_without_url_rewrite(url)?;
        }
    }
    let expected = format!("+refs/heads/*:refs/remotes/{name}/*");
    let specs = upstream.refspecs(gix::remote::Direction::Fetch);
    if branch.is_some() {
        // Pull replaces these specs with one explicit branch request below. Keep
        // custom mappings and exclusions with Git rather than silently overriding them.
        if specs.len() != 1 || specs[0].to_ref().to_bstring() != expected.as_bytes() {
            return Ok(None);
        }
    } else {
        use gix::refspec::{instruction::Fetch, Instruction};
        let destination = format!("refs/remotes/{name}/");
        let mut has_mapping = false;
        for spec in specs {
            match spec.to_ref().instruction() {
                Instruction::Fetch(Fetch::AndUpdate { src, dst, .. })
                    if src.starts_with(b"refs/heads/")
                        && dst.starts_with(destination.as_bytes()) =>
                {
                    has_mapping = true;
                }
                Instruction::Fetch(Fetch::Exclude { src }) if src.starts_with(b"refs/heads/") => {}
                _ => return Ok(None),
            }
        }
        if !has_mapping {
            return Ok(None);
        }
    }
    let merge: Vec<BString> = if let Some(branch) = branch {
        vec![format!("refs/heads/{branch}").into()]
    } else if let Some(head) = repo.head_name()? {
        let key = format!("branch.{}", head.shorten());
        if config
            .string(format!("{key}.remote").as_str())
            .as_ref()
            .map(|value| value.as_bstr())
            .is_some_and(|remote| remote == name)
        {
            config
                .plumbing()
                .strings(format!("{key}.merge").as_str())
                .unwrap_or_default()
                .into_iter()
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    if merge.iter().any(|name| !name.starts_with(b"refs/heads/")) {
        return Ok(None);
    }
    let upstream = if let Some(branch) = branch {
        let spec = format!("+refs/heads/{branch}:refs/remotes/{name}/{branch}");
        upstream.with_refspecs([spec.as_str()], gix::remote::Direction::Fetch)?
    } else if !merge.is_empty() {
        // Git also fetches the configured upstream into FETCH_HEAD when no mapping
        // covers it. Keep these source-only requests in the same group so negative
        // refspecs still apply, without inventing a tracking-reference destination.
        let specs: Vec<_> = upstream
            .refspecs(gix::remote::Direction::Fetch)
            .iter()
            .map(|spec| spec.to_ref().to_bstring())
            .chain(merge.iter().cloned())
            .collect();
        upstream.with_refspecs(
            specs.iter().map(|spec| spec.as_bstr()),
            gix::remote::Direction::Fetch,
        )?
    } else {
        upstream
    };
    // Keep FETCH_HEAD atomic, including in linked worktrees. gix updates tracking refs
    // under their own transactions but does not yet write FETCH_HEAD itself.
    let mut fetch_head = gix::lock::File::acquire_to_update_resource(
        repo.git_dir().join("FETCH_HEAD"),
        gix::lock::acquire::Fail::Immediately,
        None,
        0,
    )?;
    #[cfg(windows)]
    let connection = if upstream
        .url(gix::remote::Direction::Fetch)
        .is_some_and(|url| url.scheme == gix::url::Scheme::File)
    {
        // gix canonicalizes local repositories to extended Windows paths, which
        // its file transport cannot parse. Simplify after URL validation.
        let (mut url, version) =
            upstream.sanitized_url_and_version(gix::remote::Direction::Fetch)?;
        url.path =
            gix::path::into_bstr(dunce::simplified(&gix::path::from_bstr(&url.path)?).to_owned())?
                .into_owned();
        let transport = gix::protocol::transport::client::blocking_io::connect::connect(
            url,
            gix::protocol::transport::client::blocking_io::connect::Options {
                version,
                ..Default::default()
            },
        )?;
        upstream.to_connection_with_transport(transport)
    } else {
        upstream.connect(gix::remote::Direction::Fetch)?
    };
    #[cfg(not(windows))]
    let connection = upstream.connect(gix::remote::Direction::Fetch)?;
    let outcome = connection
        .prepare_fetch(
            gix::progress::Discard,
            gix::remote::ref_map::Options {
                // HEAD is outside the branch-derived prefix. Read its advertised
                // symref without adding a fetch mapping or a FETCH_HEAD entry for it.
                prefix_from_spec_as_filter_on_remote: !update_head,
                ..Default::default()
            },
        )?
        .receive(gix::progress::Discard, &AtomicBool::new(false))?;
    use gix::remote::fetch::{refs::update::Mode, Status};
    let updates = match &outcome.status {
        Status::Change { update_refs, .. } | Status::NoPackReceived { update_refs, .. } => {
            update_refs
        }
    };
    ensure!(
        updates.updates.iter().all(|update| matches!(
            update.mode,
            Mode::NoChangeNeeded
                | Mode::FastForward
                | Mode::Forced
                | Mode::New
                | Mode::ImplicitTagNotSentByRemote
        )),
        "Remote rejected a reference update"
    );
    let mut entries = Vec::new();
    let mut target = None;
    for (mapping, update) in outcome.ref_map.mappings.iter().zip(&updates.updates) {
        if update.mode == Mode::ImplicitTagNotSentByRemote {
            continue;
        }
        let (Some(id), Some(source)) = (mapping.remote.as_id(), mapping.remote.as_name()) else {
            continue;
        };
        // Auto-followed tags outside the received history have no local object.
        if repo.try_find_object(id)?.is_none() {
            continue;
        }
        let for_merge = merge.iter().any(|reference| reference.as_bstr() == source);
        if for_merge && branch.is_some() {
            let commit = repo.find_object(id)?.peel_to_commit()?;
            target = Some(history::RevisionId::parse(&commit.id.to_string())?);
        }
        let (kind, short) = if let Some(short) = source.strip_prefix(b"refs/heads/") {
            ("branch", short)
        } else if let Some(short) = source.strip_prefix(b"refs/tags/") {
            ("tag", short)
        } else {
            ("ref", source.as_bytes())
        };
        // Use the selected remote's name rather than persisting credential-bearing URLs.
        entries.push((
            !for_merge,
            source.to_owned(),
            format!(
                "{id}\t{}\t{kind} '{}' of {name}\n",
                if for_merge { "" } else { "not-for-merge" },
                short.as_bstr()
            ),
        ));
    }
    ensure!(
        branch.is_none() || target.is_some(),
        "Remote did not provide the requested branch"
    );
    entries.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    entries.dedup_by(|left, right| left.1 == right.1);
    for (_, _, entry) in entries {
        fetch_head.write_all(entry.as_bytes())?;
    }
    fetch_head.commit()?;
    if update_head {
        record_default_head(&repo, name, &outcome.ref_map)?;
    }
    Ok(Some(Outcome { target }))
}

/// Follow Git's default `create` policy. A remote HEAD is metadata, and must not
/// be guessed from commit equality or created when the advertised branch wasn't
/// mapped to the conventional tracking name by this fetch.
fn record_default_head(
    repo: &gix::Repository,
    name: &str,
    refs: &gix::remote::fetch::RefMap,
) -> Result<()> {
    use gix::protocol::handshake::Ref;
    use gix::refs::transaction::{PreviousValue, RefEdit};
    let head_name: gix::refs::FullName = format!("refs/remotes/{name}/HEAD").try_into()?;
    if repo.try_find_reference(head_name.as_bstr())?.is_some() {
        return Ok(());
    }
    let Some(target) = refs
        .remote_refs
        .iter()
        .find_map(|reference| match reference {
            Ref::Symbolic {
                full_ref_name,
                target,
                ..
            } if full_ref_name == "HEAD" => Some(target),
            _ => None,
        })
    else {
        return Ok(());
    };
    let Some(branch) = target.strip_prefix(b"refs/heads/") else {
        return Ok(());
    };
    let mut tracking = BString::from(format!("refs/remotes/{name}/"));
    tracking.extend_from_slice(branch);
    let tracking: gix::refs::FullName = tracking.try_into()?;
    if tracking == head_name
        || !refs.mappings.iter().any(|mapping| {
            mapping.remote.as_name() == Some(target.as_bstr())
                && mapping.local.as_ref().map(|name| name.as_bstr()) == Some(tracking.as_bstr())
        })
        || repo.try_find_reference(tracking.as_bstr())?.is_none()
    {
        return Ok(());
    }
    repo.edit_reference(
        RefEdit::update(
            head_name,
            gix::refs::Target::Symbolic(tracking),
            PreviousValue::MustNotExist,
            "fetch: recording remote HEAD",
        )
        .with_deref(false),
    )?;
    Ok(())
}
