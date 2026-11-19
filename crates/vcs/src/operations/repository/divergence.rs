//! Optional listing metadata: an unavailable comparison must not hide branches.
use super::*;
use std::{collections::HashMap, sync::Arc};

pub(super) struct Comparison {
    name: Arc<str>,
    base: gix::ObjectId,
    counts: HashMap<gix::ObjectId, Option<Divergence>>,
}

impl Comparison {
    pub(super) fn new(repo: &gix::Repository) -> Option<Self> {
        // A shallow boundary can hide shared ancestry and produce misleading counts.
        if repo.is_shallow().ok()? {
            return None;
        }
        let (name, base) = base(repo)?;
        Some(Self {
            name,
            base,
            counts: HashMap::new(),
        })
    }

    pub(super) fn counts(
        &mut self,
        repo: &gix::Repository,
        tip: gix::ObjectId,
    ) -> Option<Divergence> {
        self.counts
            .entry(tip)
            .or_insert_with(|| {
                let count = |tip, hidden| -> Result<usize> {
                    repo.rev_walk([tip])
                        .with_hidden([hidden])
                        .all()?
                        .try_fold(0, |count, commit| commit.map(|_| count + 1))
                        .map_err(Into::into)
                };
                let (ahead, behind) = if tip == self.base {
                    (0, 0)
                } else {
                    (count(tip, self.base).ok()?, count(self.base, tip).ok()?)
                };
                Some(Divergence {
                    base: self.name.clone(),
                    ahead,
                    behind,
                })
            })
            .clone()
    }
}

fn resolve(repo: &gix::Repository, name: &str) -> Option<(Arc<str>, gix::ObjectId)> {
    let mut reference = repo.try_find_reference(name).ok()??;
    let tip = reference.peel_to_commit().ok()?.id;
    Some((reference.name().shorten().to_str().ok()?.into(), tip))
}

// Read only ref names and fetch mappings from repository configuration. The
// listing uses reduced trust, so gix's remote APIs deliberately omit local config.
fn local_default(
    repo: &gix::Repository,
    remote: &str,
    target: &gix::refs::FullNameRef,
) -> Option<String> {
    let config = repo.config_snapshot();
    let specs =
        config
            .plumbing()
            .strings_by("remote", Some(remote.as_bytes().as_bstr()), "fetch")?;
    let specs = specs
        .iter()
        .map(|spec| gix::refspec::parse(spec.as_ref(), gix::refspec::parse::Operation::Fetch))
        .collect::<std::result::Result<Vec<_>, _>>()
        .ok()?;
    let null = repo.object_hash().null();
    let mappings = gix::refspec::MatchGroup::from_fetch_specs(specs).match_rhs(
        [gix::refspec::match_group::Item {
            full_ref_name: target.as_bstr(),
            target: &null,
            object: None,
        }]
        .into_iter(),
    );
    let mut mappings = mappings.mappings.into_iter();
    let mapping = mappings.next()?;
    if mappings.next().is_some() {
        return None;
    }
    let gix::refspec::match_group::SourceRef::FullName(name) = mapping.lhs else {
        return None;
    };
    let name = name.to_str().ok()?;
    name.starts_with("refs/heads/").then(|| name.to_owned())
}

fn base(repo: &gix::Repository) -> Option<(Arc<str>, gix::ObjectId)> {
    let remote = configured_remotes(repo).ok().and_then(|names| {
        if names.len() == 1 {
            names.into_iter().next()
        } else {
            names.into_iter().find(|name| name == "origin")
        }
    });
    if let Some(remote) = remote.as_ref()
        && let Some(head) = repo
            .try_find_reference(format!("refs/remotes/{remote}/HEAD").as_str())
            .ok()
            .flatten()
        && let Some(target) = head.target().try_name()
    {
        if let Some(base) =
            local_default(repo, remote, target).and_then(|name| resolve(repo, &name))
        {
            return Some(base);
        }
        if let Some(base) = target
            .as_bstr()
            .to_str()
            .ok()
            .and_then(|name| resolve(repo, name))
        {
            return Some(base);
        }
    }
    for name in ["refs/heads/main", "refs/heads/master"] {
        if let Some(base) = resolve(repo, name) {
            return Some(base);
        }
    }
    if let Some(remote) = remote {
        for name in ["main", "master"] {
            if let Some(base) = resolve(repo, &format!("refs/remotes/{remote}/{name}")) {
                return Some(base);
            }
        }
    }
    None
}
