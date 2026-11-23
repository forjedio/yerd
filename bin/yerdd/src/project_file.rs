//! `.yerd.toml` project files: the draft and export replies, the apply job,
//! and the write-back sync that mirrors per-site changes into an existing file.
//!
//! The schema, plan and delta logic is pure and lives in `yerd_config`; this
//! module resolves sites, reads daemon state and calls the existing handlers.
//! The apply job calls those handlers directly, never through
//! [`crate::ipc_server::respond`], so applying a file never writes it back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::watch;

use yerd_config::{
    extension_satisfied, newest_matching, normalise_module, plan, service_action, PlannedAction,
    PlannedStep, ProjectError, ProjectFile, ServiceAction, SiteSnapshot, SyncDelta,
    PROJECT_FILE_NAME,
};
use yerd_core::{Domain, PhpVersion, ProxyRule, ResponseHeader, Site, SiteKind, UpstreamTarget};
use yerd_ipc::{ErrorCode, JobState, Request, Response, ServiceRunState};
use yerd_php::Downloader;
use yerd_platform::node::InstalledNode;
use yerd_platform::PlatformDirs;

use crate::ipc_server::{canonicalize_dir, handle_mutation, internal, proxy_mutation};
use crate::state::DaemonState;

/// Prefix of an apply-job log line that reports a warning: the item was left
/// alone (or could not be checked) but the job can still succeed.
pub const WARNING_PREFIX: &str = "warning:";

/// Prefix of an apply-job log line that reports a failed item; any one fails
/// the job.
pub const FAILED_PREFIX: &str = "failed:";

/// A write-back change derived from one request. Domain requests carry the raw
/// FQDN; [`sync`] converts it under the live TLD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncChange {
    /// A change that needs no conversion.
    Delta(SyncDelta),
    /// A domain added, as the FQDN the request named.
    AddDomain(String),
    /// A domain removed, as the FQDN the request named.
    RemoveDomain(String),
}

/// The site name (lowercased) and change a request writes back to the site's
/// `.yerd.toml` when it succeeds, or `None` for a request that never syncs.
#[must_use]
pub fn sync_target(req: &Request) -> Option<(String, SyncChange)> {
    let (site, change) = match req {
        Request::SetPhp { name, version } => (name, SyncChange::Delta(SyncDelta::Php(*version))),
        Request::SetSecure { name, secure } => {
            (name, SyncChange::Delta(SyncDelta::Secure(*secure)))
        }
        Request::AddDomain { name, domain } | Request::SetPrimaryDomain { name, domain } => {
            (name, SyncChange::AddDomain(domain.clone()))
        }
        Request::RemoveDomain { name, domain } => (name, SyncChange::RemoveDomain(domain.clone())),
        Request::ResetDomains { name } => (name, SyncChange::Delta(SyncDelta::ResetDomains)),
        Request::SetResponseHeader { site, name, value } => (
            site,
            SyncChange::Delta(SyncDelta::SetHeader(ResponseHeader::new(name, value).ok()?)),
        ),
        Request::RemoveResponseHeader { site, name } => (
            site,
            SyncChange::Delta(SyncDelta::RemoveHeader(name.clone())),
        ),
        Request::AddProxyRule { site, prefix, url }
        | Request::UpdateProxyRule { site, prefix, url } => {
            let rule = UpstreamTarget::from_url_str(url)
                .and_then(|target| ProxyRule::new(prefix, target))
                .ok()?;
            (site, SyncChange::Delta(SyncDelta::SetRule(rule)))
        }
        Request::RemoveProxyRule { site, prefix } => (
            site,
            SyncChange::Delta(SyncDelta::RemoveRule(prefix.clone())),
        ),
        Request::SetNodeVersion {
            version,
            site: Some(site),
        } => (
            site,
            SyncChange::Delta(SyncDelta::Node(Some(version.trim().to_owned()))),
        ),
        Request::ClearNodeVersion { site } => (site, SyncChange::Delta(SyncDelta::Node(None))),
        _ => return None,
    };
    Some((site.to_ascii_lowercase(), change))
}

/// Writes `change` into the `.yerd.toml` of site `site`, if that file already
/// exists. A missing file is never created; a symlinked, oversized, unreadable
/// or invalid file is left untouched. Failures are logged only, so the
/// request's own reply is unaffected.
pub async fn sync(site: &str, change: SyncChange, state: &DaemonState) {
    let (found, tld) = {
        let router = state.router.read().await;
        let Some(found) = router.get(site).cloned() else {
            return;
        };
        (found, router.config().tld().to_owned())
    };
    let root = found.document_root().to_path_buf();
    let Ok(Ok(root)) = blocking(move || std::fs::canonicalize(root)).await else {
        return;
    };
    if !owns_dir(state, &found, &root).await {
        return;
    }
    let Some(delta) = change_to_delta(change, found.name(), &tld) else {
        return;
    };
    let path = root.join(PROJECT_FILE_NAME);
    let shown = path.display().to_string();
    let _guard = state.project_file_mutate.lock().await;
    match tokio::task::spawn_blocking(move || write_delta(&path, delta)).await {
        Ok(Ok(true)) => tracing::info!(path = %shown, "updated project file"),
        Ok(Ok(false)) => {}
        Ok(Err(e)) => tracing::warn!(path = %shown, error = %e, "project file not updated"),
        Err(e) => tracing::warn!(error = %e, "project file sync task failed"),
    }
}

/// Whether `site` is the one [`find_site_for_dir`] picks for canonical `dir`,
/// so a parked twin of a linked site never writes into the shared file.
async fn owns_dir(state: &DaemonState, site: &Site, dir: &Path) -> bool {
    find_site_for_dir(state, dir)
        .await
        .is_some_and(|owner| owner.name() == site.name() && owner.kind() == site.kind())
}

/// Converts a domain change under `tld`. Adding the apex is skipped (it is
/// implicit), but removing it still removes an explicitly declared apex.
fn change_to_delta(change: SyncChange, site: &str, tld: &str) -> Option<SyncDelta> {
    let apex = Domain::apex(site);
    match change {
        SyncChange::Delta(delta) => Some(delta),
        SyncChange::AddDomain(fqdn) => Domain::parse(&fqdn, tld)
            .ok()
            .filter(|d| *d != apex)
            .map(SyncDelta::AddDomain),
        SyncChange::RemoveDomain(fqdn) => {
            Domain::parse(&fqdn, tld).ok().map(SyncDelta::RemoveDomain)
        }
    }
}

fn write_delta(path: &Path, delta: SyncDelta) -> Result<bool, ProjectError> {
    let Some(mut file) = ProjectFile::load(path)? else {
        return Ok(false);
    };
    if !file.apply_delta(delta) {
        return Ok(false);
    }
    file.validate()?;
    file.save(path)?;
    Ok(true)
}

/// Reply to `ProjectConfigDraft`: the directory's `.yerd.toml` if present,
/// else a draft from the site at that directory, else defaults.
pub async fn draft(path: &Path, state: &DaemonState) -> Response {
    let dir = match resolve_dir(path).await {
        Ok(dir) => dir,
        Err(resp) => return resp,
    };
    let file_path = dir.join(PROJECT_FILE_NAME);
    let site = find_site_for_dir(state, &dir).await;
    let existing = match load(&file_path).await {
        Ok(existing) => existing,
        Err(e) => return project_error(&e),
    };
    let (file, exists) = match (existing, &site) {
        (Some(file), _) => (file, true),
        (None, Some(site)) => (
            ProjectFile::from_snapshot(&snapshot(state, site).await, None, false),
            false,
        ),
        (None, None) => (default_file(&dir, state).await, false),
    };
    Response::ProjectConfig {
        file: file_path,
        exists,
        site: site.map(|s| s.name().to_owned()),
        name: file.name.clone(),
        contents: file.to_toml(),
    }
}

/// Reply to `ExportProjectConfig`: write the site's current settings into its
/// `.yerd.toml`, keeping the file's `name`, services, extensions and a Node
/// selector that still resolves to the stored preference. A parked twin that
/// shares its folder with a linked site is refused: the file belongs to the
/// linked one.
pub async fn export(site: &str, state: &DaemonState) -> Response {
    let root = match crate::ipc_server::site_project_root(state, site).await {
        Ok(root) => root,
        Err(resp) => return resp,
    };
    let Some(found) = find_site_for_dir(state, &root).await else {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("no site at {}", root.display()),
        };
    };
    if !found.name().eq_ignore_ascii_case(site) {
        return Response::Error {
            code: ErrorCode::InvalidPath,
            message: format!(
                "{} belongs to site {}; export that site instead",
                root.display(),
                found.name()
            ),
        };
    }
    let installed = installed_node(state).await;
    let file_path = root.join(PROJECT_FILE_NAME);
    let _guard = state.project_file_mutate.lock().await;
    let Some(found) = state.router.read().await.get(found.name()).cloned() else {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("no site named '{site}'"),
        };
    };
    let current_root = found.document_root().to_path_buf();
    let still_here = blocking(move || std::fs::canonicalize(current_root).ok())
        .await
        .ok()
        .flatten()
        .is_some_and(|p| p == root);
    if !still_here {
        return Response::Error {
            code: ErrorCode::InvalidPath,
            message: format!(
                "site {} no longer serves {}; export it again",
                found.name(),
                root.display()
            ),
        };
    }
    let snap = snapshot(state, &found).await;
    let existing = match load(&file_path).await {
        Ok(existing) => existing,
        Err(e) => return project_error(&e),
    };
    let matches = node_matches(
        &installed,
        existing.as_ref().and_then(|f| f.node.as_deref()),
        snap.node_pref.as_deref(),
    );
    let file = ProjectFile::from_snapshot(&snap, existing.as_ref(), matches);
    if let Err(e) = save(&file_path, file.clone()).await {
        return project_error(&e);
    }
    Response::ProjectConfig {
        file: file_path,
        exists: true,
        site: Some(found.name().to_owned()),
        name: file.name.clone(),
        contents: file.to_toml(),
    }
}

/// Reply to `ApplyProjectConfig`: validate synchronously (writing the wizard's
/// `contents` first when given), then start the apply job.
pub async fn start_apply(
    path: PathBuf,
    contents: Option<String>,
    state: Arc<DaemonState>,
) -> Response {
    let prepared = match prepare(&path, contents, &state).await {
        Ok(prepared) => prepared,
        Err(resp) => return resp,
    };
    let dl: Arc<dyn Downloader> = Arc::new(crate::network::downloader(&state).await);
    spawn_apply(prepared, state, dl).await
}

/// A validated project file and the site name its apply reserves; the job
/// applies exactly this, never a re-read of the file.
struct Prepared {
    dir: PathBuf,
    file: ProjectFile,
    name: String,
    owner: Option<String>,
}

async fn prepare(
    path: &Path,
    contents: Option<String>,
    state: &DaemonState,
) -> Result<Prepared, Response> {
    let dir = resolve_dir(path).await?;
    let file_path = dir.join(PROJECT_FILE_NAME);
    let tld = state.router.read().await.config().tld().to_owned();
    let file = match contents {
        Some(text) => {
            let file = ProjectFile::from_toml(&text).map_err(|e| project_error(&e))?;
            file.validate_for_tld(&tld).map_err(|e| project_error(&e))?;
            let _guard = state.project_file_mutate.lock().await;
            let exists = || Response::Error {
                code: ErrorCode::AlreadyExists,
                message: format!(
                    "{} already exists; run `yerd init` again to apply it",
                    file_path.display()
                ),
            };
            let probe = file_path.clone();
            if blocking(move || std::fs::symlink_metadata(probe).is_ok()).await? {
                return Err(exists());
            }
            match save_new(&file_path, file.clone()).await {
                Ok(()) => file,
                Err(ProjectError::AlreadyExists { .. }) => return Err(exists()),
                Err(e) => return Err(project_error(&e)),
            }
        }
        None => load(&file_path)
            .await
            .map_err(|e| project_error(&e))?
            .ok_or_else(|| Response::Error {
                code: ErrorCode::NotFound,
                message: format!("no {PROJECT_FILE_NAME} in {}", dir.display()),
            })?,
    };
    file.validate_for_tld(&tld).map_err(|e| project_error(&e))?;
    let owner = find_site_for_dir(state, &dir)
        .await
        .map(|site| site.name().to_owned());
    let name = match (file.name.clone(), owner.clone()) {
        (Some(name), _) => name,
        (None, Some(site)) => site,
        (None, None) => folder_slug(&dir).ok_or_else(|| Response::Error {
            code: ErrorCode::InvalidPath,
            message: format!(
                "cannot derive a site name from {}; set `name` in {PROJECT_FILE_NAME}",
                dir.display()
            ),
        })?,
    };
    Ok(Prepared {
        dir,
        file,
        name,
        owner,
    })
}

async fn spawn_apply(
    prepared: Prepared,
    state: Arc<DaemonState>,
    dl: Arc<dyn Downloader>,
) -> Response {
    let (job_id, cancel_rx) = state.jobs.create().await;
    let id = job_id.clone();
    tokio::spawn(async move {
        run_job(id, prepared, state, dl, cancel_rx).await;
    });
    Response::JobStarted { job_id }
}

enum Outcome {
    Done { failed: usize },
    Cancelled,
    Fatal(String),
}

async fn run_job(
    id: String,
    prepared: Prepared,
    state: Arc<DaemonState>,
    dl: Arc<dyn Downloader>,
    cancel_rx: watch::Receiver<bool>,
) {
    let Prepared {
        dir,
        file,
        name,
        owner,
    } = prepared;
    let reserved: Vec<String> = std::iter::once(name.clone())
        .chain(owner.filter(|o| *o != name))
        .collect();
    if !reserve_all(&state, &reserved).await {
        state
            .jobs
            .finish(
                &id,
                JobState::Failed,
                Some(format!(
                    "a site named \"{name}\" is already being created or applied"
                )),
            )
            .await;
        return;
    }
    let outcome = run_apply(&id, &dir, &file, &name, &state, dl.as_ref(), &cancel_rx).await;
    release_all(&state, &reserved).await;
    match outcome {
        Outcome::Done { failed: 0 } => {
            state.jobs.set_phase(&id, "Done").await;
            state.jobs.finish(&id, JobState::Succeeded, None).await;
        }
        Outcome::Done { failed } => {
            let message = format!("{failed} item(s) could not be applied");
            state
                .jobs
                .finish(&id, JobState::Failed, Some(message))
                .await;
        }
        Outcome::Cancelled => {
            state.jobs.push_log(&id, "cancelled".to_owned()).await;
            state.jobs.finish(&id, JobState::Cancelled, None).await;
        }
        Outcome::Fatal(message) => {
            state.jobs.push_log(&id, format!("error: {message}")).await;
            state
                .jobs
                .finish(&id, JobState::Failed, Some(message))
                .await;
        }
    }
}

fn is_cancelled(cancel_rx: &watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow()
}

async fn run_apply(
    id: &str,
    dir: &Path,
    file: &ProjectFile,
    name: &str,
    state: &Arc<DaemonState>,
    dl: &dyn Downloader,
    cancel_rx: &watch::Receiver<bool>,
) -> Outcome {
    if is_cancelled(cancel_rx) {
        return Outcome::Cancelled;
    }
    let mut ap = Applier {
        id,
        state,
        dl,
        counts: Counts::default(),
    };
    state.jobs.set_phase(id, "Linking").await;
    let site = match ap.resolve_site(file, dir, name).await {
        Ok(site) => site,
        Err(message) => return Outcome::Fatal(message),
    };
    let site_name = site.name().to_owned();
    let snap = snapshot(state, &site).await;
    let installed = installed_node(state).await;
    let node_unchanged = node_matches(&installed, file.node.as_deref(), snap.node_pref.as_deref());
    let tld = state.router.read().await.config().tld().to_owned();
    state.jobs.set_phase(id, "Applying").await;
    for step in plan(file, Some(&snap), node_unchanged) {
        if is_cancelled(cancel_rx) {
            return Outcome::Cancelled;
        }
        ap.step(&site_name, dir, &tld, step).await;
    }
    if is_cancelled(cancel_rx) {
        return Outcome::Cancelled;
    }
    if !ap.services(&file.services, cancel_rx).await {
        return Outcome::Cancelled;
    }
    if !ap.extensions(&site_name, &file.extensions, cancel_rx).await {
        return Outcome::Cancelled;
    }
    let c = &ap.counts;
    state
        .jobs
        .push_log(
            id,
            format!(
                "done: {} applied, {} unchanged, {} warning(s), {} failed",
                c.applied, c.unchanged, c.warnings, c.failed
            ),
        )
        .await;
    Outcome::Done { failed: c.failed }
}

#[derive(Default)]
struct Counts {
    applied: usize,
    unchanged: usize,
    warnings: usize,
    failed: usize,
}

struct Applier<'a> {
    id: &'a str,
    state: &'a Arc<DaemonState>,
    dl: &'a dyn Downloader,
    counts: Counts,
}

impl Applier<'_> {
    async fn line(&self, line: String) {
        self.state.jobs.push_log(self.id, line).await;
    }

    async fn applied(&mut self, verb: &str, what: impl std::fmt::Display) {
        self.counts.applied += 1;
        self.line(format!("{verb}: {what}")).await;
    }

    async fn unchanged(&mut self, what: impl std::fmt::Display) {
        self.counts.unchanged += 1;
        self.line(format!("unchanged: {what}")).await;
    }

    async fn warn(&mut self, what: impl std::fmt::Display) {
        self.counts.warnings += 1;
        self.line(format!("{WARNING_PREFIX} {what}")).await;
    }

    async fn fail(&mut self, what: impl std::fmt::Display) {
        self.counts.failed += 1;
        self.line(format!("{FAILED_PREFIX} {what}")).await;
    }

    async fn report(&mut self, label: String, resp: Response) -> bool {
        match resp {
            Response::Ok => {
                self.applied("applied", label).await;
                true
            }
            Response::Error {
                code: ErrorCode::AlreadyExists,
                message,
            } => {
                self.warn(format!("{label}: {message}; skipped")).await;
                false
            }
            Response::Error { message, .. } => {
                self.fail(format!("{label}: {message}")).await;
                false
            }
            other => {
                self.fail(format!("{label}: unexpected reply {other:?}"))
                    .await;
                false
            }
        }
    }

    async fn resolve_site(
        &mut self,
        file: &ProjectFile,
        dir: &Path,
        name: &str,
    ) -> Result<Site, String> {
        let state = self.state;
        match find_site_for_dir(state, dir).await {
            None => {
                link(state, name, dir).await?;
                self.applied("applied", format!("linked {name}")).await;
            }
            Some(site) if site.kind() == SiteKind::Linked => {
                match &file.name {
                    Some(declared) if declared != site.name() => {
                        self.warn(format!(
                            "this folder is linked as {}, so `name = \"{declared}\"` is not applied",
                            site.name()
                        ))
                        .await;
                    }
                    _ => self.unchanged(format!("site {}", site.name())).await,
                }
                return Ok(site);
            }
            Some(parked) => match &file.name {
                Some(declared) if declared != parked.name() => {
                    if let Err(message) = link(state, declared, dir).await {
                        self.fail(format!("link as {declared}: {message}")).await;
                        return Ok(parked);
                    }
                    self.applied("applied", format!("linked {declared}")).await;
                    self.keep_parked_settings(file, declared, &parked).await;
                }
                _ => {
                    self.unchanged(format!("site {}", parked.name())).await;
                    return Ok(parked);
                }
            },
        }
        find_site_for_dir(state, dir)
            .await
            .ok_or_else(|| format!("{} was linked but is not routed", dir.display()))
    }

    async fn keep_parked_settings(&mut self, file: &ProjectFile, name: &str, parked: &Site) {
        let Some(linked) = self.state.router.read().await.get(name).cloned() else {
            return;
        };
        if file.php.is_none() && linked.php() != parked.php() {
            let resp = handle_mutation(
                Request::SetPhp {
                    name: name.to_owned(),
                    version: parked.php(),
                },
                self.state,
            )
            .await;
            self.report(
                format!("PHP {} (kept from the parked site)", parked.php()),
                resp,
            )
            .await;
        }
        if file.secure.is_none() && linked.secure() != parked.secure() {
            let resp = handle_mutation(
                Request::SetSecure {
                    name: name.to_owned(),
                    secure: parked.secure(),
                },
                self.state,
            )
            .await;
            self.report(
                format!(
                    "{} (kept from the parked site)",
                    https_label(parked.secure())
                ),
                resp,
            )
            .await;
        }
        let key = parked.document_root().to_string_lossy().into_owned();
        let web_root = self
            .state
            .config
            .lock()
            .await
            .overrides
            .get(&key)
            .and_then(|o| o.web_root.clone());
        if let Some(rel) = web_root.filter(|rel| linked.web_subpath() != Path::new(rel)) {
            let resp = handle_mutation(
                Request::SetWebRoot {
                    name: name.to_owned(),
                    path: Some(rel.clone()),
                },
                self.state,
            )
            .await;
            let shown = if rel.is_empty() { "." } else { rel.as_str() };
            self.report(
                format!("web root {shown} (kept from the parked site)"),
                resp,
            )
            .await;
        }
    }

    async fn step(&mut self, site: &str, dir: &Path, tld: &str, step: PlannedStep) {
        let PlannedStep { action, needed } = step;
        match action {
            PlannedAction::Link { .. } => {}
            PlannedAction::Php(version) => self.php(site, version, needed).await,
            PlannedAction::Secure(secure) => {
                let label = https_label(secure).to_owned();
                let req = Request::SetSecure {
                    name: site.to_owned(),
                    secure,
                };
                self.mutation(label, req, needed).await;
            }
            PlannedAction::Domain(domain) => {
                let fqdn = domain.to_fqdn(tld);
                let req = Request::AddDomain {
                    name: site.to_owned(),
                    domain: fqdn.clone(),
                };
                self.mutation(format!("domain {fqdn}"), req, needed).await;
            }
            PlannedAction::Header(header) => {
                let req = Request::SetResponseHeader {
                    site: site.to_owned(),
                    name: header.name().to_owned(),
                    value: header.value().to_owned(),
                };
                self.mutation(format!("header {}", header.name()), req, needed)
                    .await;
            }
            PlannedAction::ProxyRule { rule, update } => {
                let label = format!("proxy rule {} -> {}", rule.prefix(), rule.target());
                if !needed {
                    self.unchanged(label).await;
                    return;
                }
                let (site, prefix, url) = (
                    site.to_owned(),
                    rule.prefix().to_owned(),
                    rule.target().to_string(),
                );
                let req = if update {
                    Request::UpdateProxyRule { site, prefix, url }
                } else {
                    Request::AddProxyRule { site, prefix, url }
                };
                let resp = proxy_mutation(req, self.state).await;
                self.report(label, resp).await;
            }
            PlannedAction::Node(selector) => self.node(site, dir, &selector, needed).await,
        }
    }

    async fn mutation(&mut self, label: String, req: Request, needed: bool) {
        if needed {
            let resp = handle_mutation(req, self.state).await;
            self.report(label, resp).await;
        } else {
            self.unchanged(label).await;
        }
    }

    async fn php(&mut self, site: &str, version: PhpVersion, needed: bool) {
        let label = format!("PHP {version}");
        if !crate::php_install::cli_binary_path(&self.state.dirs, version).is_file() {
            if version.is_legacy() {
                self.fail(format!(
                    "{label} is a legacy version and is not installed; run \
                     `yerd install php {version} --legacy`, then `yerd init` again"
                ))
                .await;
                return;
            }
            self.state
                .jobs
                .set_phase(self.id, format!("Installing {label}"))
                .await;
            let resp =
                crate::ipc_server::install_php_with(version, false, self.state, self.dl).await;
            match resp {
                Response::Ok => self.applied("installed", &label).await,
                Response::Error { message, .. } => {
                    self.fail(format!("install {label}: {message}")).await;
                    return;
                }
                other => {
                    self.fail(format!("install {label}: unexpected reply {other:?}"))
                        .await;
                    return;
                }
            }
        }
        let req = Request::SetPhp {
            name: site.to_owned(),
            version,
        };
        self.mutation(label, req, needed).await;
    }

    async fn node(&mut self, site: &str, dir: &Path, selector: &str, needed: bool) {
        let label = format!("Node {selector}");
        if needed {
            if yerd_platform::node::resolve(&self.state.dirs, selector).is_err() {
                self.state
                    .jobs
                    .set_phase(self.id, format!("Installing {label}"))
                    .await;
                match crate::ipc_server::install_node_with(selector, self.state, self.dl).await {
                    Response::Ok => self.applied("installed", &label).await,
                    Response::Error { message, .. } => {
                        self.fail(format!("install {label}: {message}")).await;
                        return;
                    }
                    other => {
                        self.fail(format!("install {label}: unexpected reply {other:?}"))
                            .await;
                        return;
                    }
                }
            }
            let resp = crate::ipc_server::set_node_version(selector, Some(site), self.state).await;
            self.report(label, resp).await;
        } else {
            self.unchanged(label).await;
        }
        if let Ok(Some(pinned)) = yerd_platform::node::project_file(dir, Some(dir)) {
            if !same_node_selector(&pinned.selector, selector) {
                self.warn(format!(
                    "{} pins Node {}, which wins over node = \"{selector}\"",
                    pinned.kind.file_name(),
                    pinned.selector
                ))
                .await;
            }
        }
    }

    /// Brings each declared service in line; `false` when cancelled first.
    async fn services(
        &mut self,
        services: &BTreeMap<String, String>,
        cancel_rx: &watch::Receiver<bool>,
    ) -> bool {
        if services.is_empty() {
            return !is_cancelled(cancel_rx);
        }
        let statuses = crate::services::service_statuses(self.state).await;
        for (engine, prefix) in services {
            if is_cancelled(cancel_rx) {
                return false;
            }
            let status = statuses.iter().find(|s| s.service == *engine);
            let installed = status
                .map(|s| s.installed_versions.clone())
                .unwrap_or_default();
            let selected = status.and_then(|s| s.selected_version.clone());
            let running = status.is_some_and(|s| s.state == ServiceRunState::Running);
            let available = if installed.is_empty() && selected.is_none() {
                let labels = crate::services::available_labels(engine, self.dl).await;
                if is_cancelled(cancel_rx) {
                    return false;
                }
                match labels {
                    Ok(labels) => labels,
                    Err(message) => {
                        self.fail(format!("{engine}: {message}")).await;
                        continue;
                    }
                }
            } else {
                Vec::new()
            };
            match service_action(prefix, &installed, selected.as_deref(), &available, running) {
                ServiceAction::Satisfied { start: false } => {
                    self.unchanged(format!("{engine} {prefix}")).await;
                }
                ServiceAction::Satisfied { start: true } => {
                    let version = match selected {
                        Some(_) => None,
                        None => newest_matching(prefix, &installed),
                    };
                    match crate::services::start_service_at(engine, version, self.state).await {
                        Response::Ok => self.applied("started", engine).await,
                        Response::Error { message, .. } => {
                            self.fail(format!("start {engine}: {message}")).await;
                        }
                        other => {
                            self.fail(format!("start {engine}: unexpected reply {other:?}"))
                                .await;
                        }
                    }
                }
                ServiceAction::Install(label) => self.install_service(engine, &label).await,
                ServiceAction::Mismatch => {
                    let current = selected.clone().unwrap_or_else(|| installed.join(", "));
                    self.warn(format!(
                        "{engine} {current} is installed but {PROJECT_FILE_NAME} asks for {prefix}; \
                         switch with `yerd service change-version {engine} <version>`"
                    ))
                    .await;
                }
                ServiceAction::Unavailable => {
                    self.fail(format!(
                        "no {engine} version matching {prefix} is available for this platform"
                    ))
                    .await;
                }
            }
        }
        true
    }

    async fn install_service(&mut self, engine: &str, label: &str) {
        self.state
            .jobs
            .set_phase(self.id, format!("Installing {engine} {label}"))
            .await;
        let resp = crate::services::add_service(
            engine,
            None,
            None,
            Some(label),
            None,
            self.state,
            self.dl,
        )
        .await;
        crate::ipc_server::reconcile_client_shims_now(self.state).await;
        match resp {
            Response::ServiceInstanceId { .. } => {
                self.applied("installed", format!("{engine} {label}")).await;
            }
            Response::Error {
                code: ErrorCode::InstanceAlreadyExists,
                message,
            } => {
                self.fail(format!(
                    "{engine}: {message} (clear a stale entry with `yerd service remove {engine}`)"
                ))
                .await;
            }
            Response::Error { message, .. } => {
                self.fail(format!("install {engine} {label}: {message}"))
                    .await;
            }
            other => {
                self.fail(format!(
                    "install {engine} {label}: unexpected reply {other:?}"
                ))
                .await;
            }
        }
    }

    /// Checks each required extension; `false` when cancelled first.
    async fn extensions(
        &mut self,
        site: &str,
        names: &[String],
        cancel_rx: &watch::Receiver<bool>,
    ) -> bool {
        if is_cancelled(cancel_rx) {
            return false;
        }
        if names.is_empty() {
            return true;
        }
        let Some(php) = self.state.router.read().await.get(site).map(Site::php) else {
            return true;
        };
        let bin = crate::php_install::cli_binary_path(&self.state.dirs, php);
        if !bin.is_file() {
            self.warn(format!(
                "cannot check extensions: PHP {php} is not installed"
            ))
            .await;
            return true;
        }
        let runner = self.state.ffi_cache.runner();
        let probed = yerd_php::probe_modules(runner.as_ref(), &bin).await;
        if is_cancelled(cancel_rx) {
            return false;
        }
        let Some(compiled) = probed else {
            self.warn(format!(
                "cannot check extensions: PHP {php} did not list its modules"
            ))
            .await;
            return true;
        };
        let (custom, xdebug_on) = {
            let cfg = self.state.config.lock().await;
            let custom: Vec<String> = cfg
                .php
                .extensions
                .get(&php)
                .map(|entries| entries.iter().map(|e| e.name.clone()).collect())
                .unwrap_or_default();
            (custom, cfg.php.xdebug.get(&php).is_some_and(|x| x.enabled))
        };
        let pcov = crate::ext_install::pcov_so_path(&self.state.dirs, php).is_file();
        for name in names {
            if is_cancelled(cancel_rx) {
                return false;
            }
            if extension_satisfied(name, &compiled, &custom, xdebug_on) {
                self.unchanged(format!("extension {name}")).await;
            } else if normalise_module(name) == PCOV && pcov {
                self.unchanged(format!(
                    "extension {name} (CLI only: run tests with `phpcover`)"
                ))
                .await;
            } else if name == "xdebug" {
                self.warn(format!(
                    "extension xdebug is off for PHP {php}; turn it on with `yerd xdebug on {php}`"
                ))
                .await;
            } else {
                self.warn(format!(
                    "extension {name} is not available in PHP {php}; add it with \
                     `yerd php ext add {php} <path-to.so>`"
                ))
                .await;
            }
        }
        true
    }
}

/// The bundled coverage extension, loaded only by the `phpcover` CLI shims.
const PCOV: &str = "pcov";

fn https_label(secure: bool) -> &'static str {
    if secure {
        "HTTPS on"
    } else {
        "HTTPS off"
    }
}

async fn link(state: &DaemonState, name: &str, dir: &Path) -> Result<(), String> {
    let resp = handle_mutation(
        Request::Link {
            name: name.to_owned(),
            path: dir.to_path_buf(),
        },
        state,
    )
    .await;
    match resp {
        Response::Ok => Ok(()),
        Response::Error { message, .. } => Err(message),
        other => Err(format!("unexpected reply {other:?}")),
    }
}

/// The site serving canonical directory `dir`. When a parked twin shares the
/// directory with a linked site (left after a parked folder is linked under
/// another name), the linked one wins.
/// Reserves every name in `names` or none of them, so two applies that target
/// the same site (by declared name or by the site already at the folder) never
/// run at once.
async fn reserve_all(state: &DaemonState, names: &[String]) -> bool {
    let mut reserved = state.reserved_names.lock().await;
    if names.iter().any(|n| reserved.contains(n)) {
        return false;
    }
    reserved.extend(names.iter().cloned());
    true
}

async fn release_all(state: &DaemonState, names: &[String]) {
    let mut reserved = state.reserved_names.lock().await;
    for n in names {
        reserved.remove(n);
    }
}

async fn find_site_for_dir(state: &DaemonState, dir: &Path) -> Option<Site> {
    let sites: Vec<Site> = state.router.read().await.iter().cloned().collect();
    let dir = dir.to_path_buf();
    blocking(move || {
        let mut matching: Vec<Site> = sites
            .into_iter()
            .filter(|s| std::fs::canonicalize(s.document_root()).is_ok_and(|p| p == dir))
            .collect();
        matching.sort_by_key(|s| s.kind() != SiteKind::Linked);
        matching.into_iter().next()
    })
    .await
    .ok()
    .flatten()
}

async fn resolve_dir(path: &Path) -> Result<PathBuf, Response> {
    let path = path.to_path_buf();
    blocking(move || canonicalize_dir(&path)).await?
}

/// Runs blocking filesystem work off the async runtime; a panicked or
/// cancelled task becomes an internal error reply.
async fn blocking<T, F>(f: F) -> Result<T, Response>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| internal(e.to_string()))
}

async fn snapshot(state: &DaemonState, site: &Site) -> SiteSnapshot {
    let root = site.document_root().to_path_buf();
    let canonical = blocking(move || std::fs::canonicalize(&root).unwrap_or(root))
        .await
        .unwrap_or_else(|_| site.document_root().to_path_buf());
    let name = site.name();
    let parked_key = site.document_root().to_string_lossy().into_owned();
    let linked = site.kind() == SiteKind::Linked;
    let cfg = state.config.lock().await;
    let domain_delta = entry_for(
        linked,
        name,
        &parked_key,
        &cfg.domains.linked,
        &cfg.domains.parked,
    );
    let domains_added = domain_delta.map(|d| d.added.clone()).unwrap_or_default();
    let apex_suppressed = domain_delta.is_some_and(|d| d.suppressed.contains(&Domain::apex(name)));
    let headers = entry_for(
        linked,
        name,
        &parked_key,
        &cfg.response_headers.linked,
        &cfg.response_headers.parked,
    )
    .cloned()
    .unwrap_or_default();
    let proxy_rules = entry_for(
        linked,
        name,
        &parked_key,
        &cfg.proxy_rules.linked,
        &cfg.proxy_rules.parked,
    )
    .map(|rules| {
        rules
            .iter()
            .filter(|r| crate::mutate::managed_rule_service(&cfg, name, r.prefix()).is_none())
            .cloned()
            .collect()
    })
    .unwrap_or_default();
    SiteSnapshot {
        name: name.to_owned(),
        php: site.php(),
        secure: site.secure(),
        domains_added,
        apex_suppressed,
        headers,
        proxy_rules,
        node_pref: cfg
            .node
            .sites
            .get(canonical.to_string_lossy().as_ref())
            .cloned(),
    }
}

fn entry_for<'a, T>(
    linked: bool,
    name: &str,
    parked_key: &str,
    linked_map: &'a BTreeMap<String, T>,
    parked_map: &'a BTreeMap<String, T>,
) -> Option<&'a T> {
    if linked {
        linked_map.get(name)
    } else {
        parked_map.get(parked_key)
    }
}

/// Whether two Node selectors name the same thing once trimmed, with a leading
/// `v`/`V` dropped and case ignored, so `.nvmrc`'s `v22` matches `node = "22"`.
fn same_node_selector(a: &str, b: &str) -> bool {
    fn norm(s: &str) -> String {
        let s = s.trim();
        s.strip_prefix(['v', 'V']).unwrap_or(s).to_ascii_lowercase()
    }
    norm(a) == norm(b)
}

/// Whether the stored exact preference `pref` is an installed release that
/// `selector` matches. Any matching release counts, not only the newest, so
/// installing a newer `22.x` leaves `node = "22"` pinned at `22.11.0` unchanged.
fn node_matches(installed: &[InstalledNode], selector: Option<&str>, pref: Option<&str>) -> bool {
    let (Some(selector), Some(pref)) = (selector, pref) else {
        return false;
    };
    yerd_core::node::select_release(
        selector,
        installed
            .iter()
            .filter(|n| n.version.strip_prefix('v').unwrap_or(&n.version) == pref)
            .map(yerd_platform::node::InstalledNode::candidate),
    )
    .is_ok_and(|exact| exact == pref)
}

/// The installed Node releases, read on the blocking pool.
async fn installed_node(state: &DaemonState) -> Vec<InstalledNode> {
    let dirs: PlatformDirs = state.dirs.clone();
    blocking(move || yerd_platform::node::installed(&dirs))
        .await
        .unwrap_or_default()
}

async fn default_file(dir: &Path, state: &DaemonState) -> ProjectFile {
    ProjectFile {
        name: folder_slug(dir),
        php: Some(state.config.lock().await.php.default),
        ..ProjectFile::default()
    }
}

fn folder_slug(dir: &Path) -> Option<String> {
    dir.file_name()
        .and_then(|n| n.to_str())
        .and_then(yerd_core::slugify_site_name)
}

async fn load(path: &Path) -> Result<Option<ProjectFile>, ProjectError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || ProjectFile::load(&path))
        .await
        .unwrap_or_else(|e| {
            Err(ProjectError::Io {
                path: PathBuf::new(),
                source: std::io::Error::other(e.to_string()),
            })
        })
}

async fn save(path: &Path, file: ProjectFile) -> Result<(), ProjectError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || file.save(&path))
        .await
        .unwrap_or_else(|e| {
            Err(ProjectError::Io {
                path: PathBuf::new(),
                source: std::io::Error::other(e.to_string()),
            })
        })
}

async fn save_new(path: &Path, file: ProjectFile) -> Result<(), ProjectError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || file.save_new(&path))
        .await
        .unwrap_or_else(|e| {
            Err(ProjectError::Io {
                path: PathBuf::new(),
                source: std::io::Error::other(e.to_string()),
            })
        })
}

fn project_error(e: &ProjectError) -> Response {
    match e {
        ProjectError::Io { .. } => internal(e.to_string()),
        ProjectError::AlreadyExists { .. } => Response::Error {
            code: ErrorCode::AlreadyExists,
            message: e.to_string(),
        },
        _ => Response::Error {
            code: ErrorCode::ProjectFileInvalid,
            message: e.to_string(),
        },
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::test_support::{fake_php, state_in, UrlDl};

    fn php_cli(state: &DaemonState, version: &str) {
        fake_php(&state.dirs, version);
        let bin = crate::php_install::cli_binary_path(&state.dirs, version.parse().unwrap());
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(bin, b"x").unwrap();
    }

    fn node_release(state: &DaemonState, version: &str) {
        let root = yerd_platform::node::versions_dir(&state.dirs).join(version);
        let bin = root.join(format!("node-v{version}-test/bin"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(root.join(".version"), version).unwrap();
        for name in ["node", "npm", "npx"] {
            std::fs::write(bin.join(name), "fake").unwrap();
        }
    }

    fn project(tmp: &Path, name: &str, contents: Option<&str>) -> PathBuf {
        let dir = tmp.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(text) = contents {
            std::fs::write(dir.join(PROJECT_FILE_NAME), text).unwrap();
        }
        std::fs::canonicalize(dir).unwrap()
    }

    fn ok(resp: Response) {
        assert!(matches!(resp, Response::Ok), "got {resp:?}");
    }

    async fn link_site(state: &DaemonState, name: &str, dir: &Path) {
        ok(handle_mutation(
            Request::Link {
                name: name.to_owned(),
                path: dir.to_path_buf(),
            },
            state,
        )
        .await);
    }

    fn offline() -> Arc<dyn Downloader> {
        Arc::new(UrlDl::default())
    }

    async fn apply(
        state: &Arc<DaemonState>,
        dir: &Path,
    ) -> (JobState, Vec<String>, Option<String>) {
        let prepared = prepare(dir, None, state).await.unwrap();
        let Response::JobStarted { job_id } = spawn_apply(prepared, state.clone(), offline()).await
        else {
            panic!("expected JobStarted");
        };
        wait(state, &job_id).await
    }

    async fn wait(state: &DaemonState, job_id: &str) -> (JobState, Vec<String>, Option<String>) {
        for _ in 0..500 {
            match state.jobs.poll(job_id, 0).await {
                Response::JobProgress {
                    state: JobState::Running,
                    ..
                } => tokio::time::sleep(Duration::from_millis(10)).await,
                Response::JobProgress {
                    state, log, error, ..
                } => return (state, log, error),
                other => panic!("unexpected {other:?}"),
            }
        }
        panic!("job did not finish");
    }

    async fn site_of(state: &DaemonState, name: &str) -> Site {
        state.router.read().await.get(name).cloned().unwrap()
    }

    fn code_of(resp: &Response) -> ErrorCode {
        match resp {
            Response::Error { code, .. } => *code,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    fn contents_of(resp: Response) -> (bool, Option<String>, Option<String>, String) {
        match resp {
            Response::ProjectConfig {
                exists,
                site,
                name,
                contents,
                ..
            } => (exists, site, name, contents),
            other => panic!("expected ProjectConfig, got {other:?}"),
        }
    }

    const FULL: &str = "name = \"acme\"\nphp = \"8.4\"\nsecure = true\nnode = \"22\"\n\
                        domains = [\"admin.acme\"]\n\n[headers]\nX-Frame-Options = \"DENY\"\n\n\
                        [proxy_rules]\n\"/app\" = \"http://127.0.0.1:3000\"\n";

    #[tokio::test]
    async fn draft_for_an_unknown_dir_uses_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dir = project(tmp.path(), "My App", None);
        let (exists, site, name, contents) = contents_of(draft(&dir, &state).await);
        assert!(!exists);
        assert_eq!(site, None);
        assert_eq!(name.as_deref(), Some("my-app"));
        assert!(contents.contains("php = \"8.3\""), "{contents}");
    }

    #[tokio::test]
    async fn draft_for_a_linked_site_reflects_its_state() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dir = project(tmp.path(), "acme", None);
        link_site(&state, "acme", &dir).await;
        ok(handle_mutation(
            Request::SetSecure {
                name: "acme".into(),
                secure: true,
            },
            &state,
        )
        .await);
        let (exists, site, _, contents) = contents_of(draft(&dir, &state).await);
        assert!(!exists);
        assert_eq!(site.as_deref(), Some("acme"));
        assert!(contents.contains("secure = true"), "{contents}");
        assert!(!dir.join(PROJECT_FILE_NAME).exists());
    }

    #[tokio::test]
    async fn draft_returns_an_existing_file_or_its_error() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let good = project(tmp.path(), "good", Some("name = \"shop\"\nphp = \"8.2\"\n"));
        let (exists, _, name, contents) = contents_of(draft(&good, &state).await);
        assert!(exists);
        assert_eq!(name.as_deref(), Some("shop"));
        assert!(contents.contains("php = \"8.2\""), "{contents}");

        let bad = project(tmp.path(), "bad", Some("colour = \"blue\"\n"));
        assert_eq!(
            code_of(&draft(&bad, &state).await),
            ErrorCode::ProjectFileInvalid
        );
    }

    #[tokio::test]
    async fn draft_prefers_the_linked_site_over_a_parked_twin() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let parent = tmp.path().join("Sites");
        let dir = project(&parent, "foo", None);
        ok(handle_mutation(Request::Park { path: parent }, &state).await);
        link_site(&state, "acme", &dir).await;
        assert!(state.router.read().await.get("foo").is_some());
        let (_, site, _, _) = contents_of(draft(&dir, &state).await);
        assert_eq!(site.as_deref(), Some("acme"));
    }

    #[tokio::test]
    async fn export_creates_the_file_and_is_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dir = project(tmp.path(), "acme", None);
        link_site(&state, "acme", &dir).await;
        ok(handle_mutation(
            Request::AddDomain {
                name: "acme".into(),
                domain: "api.acme.test".into(),
            },
            &state,
        )
        .await);
        let (exists, _, _, _) = contents_of(export("acme", &state).await);
        assert!(exists);
        let first = std::fs::read(dir.join(PROJECT_FILE_NAME)).unwrap();
        assert!(String::from_utf8_lossy(&first).contains("domains = [\"api.acme\"]"));
        contents_of(export("acme", &state).await);
        assert_eq!(std::fs::read(dir.join(PROJECT_FILE_NAME)).unwrap(), first);
        assert_eq!(code_of(&export("nope", &state).await), ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn export_keeps_services_and_a_matching_node_selector() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        node_release(&state, "22.11.0");
        let dir = project(
            tmp.path(),
            "acme",
            Some("node = \"22\"\n\n[services]\nmysql = \"8.4\"\n"),
        );
        link_site(&state, "acme", &dir).await;
        ok(crate::ipc_server::set_node_version("22", Some("acme"), &state).await);
        let (_, _, _, contents) = contents_of(export("acme", &state).await);
        assert!(contents.contains("node = \"22\""), "{contents}");
        assert!(contents.contains("mysql = \"8.4\""), "{contents}");
        assert!(contents.contains("name = \"acme\""), "{contents}");
    }

    #[test]
    fn node_matches_any_installed_release_the_selector_covers() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        node_release(&state, "22.11.0");
        node_release(&state, "22.12.0");
        node_release(&state, "20.18.0");
        let installed = yerd_platform::node::installed(&state.dirs);
        let cases: &[(Option<&str>, Option<&str>, bool)] = &[
            (Some("22"), Some("22.11.0"), true),
            (Some("22"), Some("22.12.0"), true),
            (Some("22.11"), Some("22.11.0"), true),
            (Some("22"), Some("20.18.0"), false),
            (Some("22"), Some("22.10.0"), false),
            (Some("20"), Some("22.11.0"), false),
            (Some("node"), Some("20.18.0"), true),
            (None, Some("22.11.0"), false),
            (Some("22"), None, false),
        ];
        for (selector, pref, want) in cases {
            assert_eq!(
                node_matches(&installed, *selector, *pref),
                *want,
                "{selector:?} {pref:?}"
            );
        }
    }

    #[tokio::test]
    async fn export_keeps_the_selector_after_a_newer_matching_install() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        node_release(&state, "22.11.0");
        let dir = project(tmp.path(), "acme", Some("node = \"22\"\n"));
        link_site(&state, "acme", &dir).await;
        ok(crate::ipc_server::set_node_version("22", Some("acme"), &state).await);
        node_release(&state, "22.12.0");
        let (_, _, _, contents) = contents_of(export("acme", &state).await);
        assert!(contents.contains("node = \"22\""), "{contents}");
        assert_eq!(
            state
                .config
                .lock()
                .await
                .node
                .sites
                .get(dir.to_string_lossy().as_ref())
                .map(String::as_str),
            Some("22.11.0")
        );
    }

    #[tokio::test]
    async fn apply_links_a_fresh_folder_then_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        php_cli(&state, "8.4");
        node_release(&state, "22.11.0");
        let dir = project(tmp.path(), "checkout", Some(FULL));
        let before = std::fs::read(dir.join(PROJECT_FILE_NAME)).unwrap();

        let (job, log, error) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded, "{log:?} {error:?}");
        let site = site_of(&state, "acme").await;
        assert_eq!(site.php(), "8.4".parse().unwrap());
        assert!(site.secure());
        let cfg = state.config.lock().await.clone();
        assert_eq!(
            cfg.domains.linked["acme"].added,
            vec![Domain::parse_subpart("admin.acme").unwrap()]
        );
        assert_eq!(cfg.response_headers.linked["acme"].len(), 1);
        assert_eq!(cfg.proxy_rules.linked["acme"].len(), 1);
        assert_eq!(
            cfg.node
                .sites
                .get(dir.to_string_lossy().as_ref())
                .map(String::as_str),
            Some("22.11.0")
        );

        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded);
        let items: Vec<&String> = log.iter().filter(|l| !l.starts_with("done:")).collect();
        assert_eq!(items.len(), 7, "{log:?}");
        assert!(items.iter().all(|l| l.starts_with("unchanged:")), "{log:?}");
        assert_eq!(std::fs::read(dir.join(PROJECT_FILE_NAME)).unwrap(), before);
    }

    #[tokio::test]
    async fn apply_links_a_parked_folder_under_the_declared_name() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        php_cli(&state, "8.4");
        let parent = tmp.path().join("Sites");
        let dir = project(&parent, "foo", Some("name = \"acme\"\n"));
        ok(handle_mutation(Request::Park { path: parent }, &state).await);
        ok(handle_mutation(
            Request::SetPhp {
                name: "foo".into(),
                version: "8.4".parse().unwrap(),
            },
            &state,
        )
        .await);

        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded, "{log:?}");
        let site = site_of(&state, "acme").await;
        assert_eq!(site.kind(), SiteKind::Linked);
        assert_eq!(site.php(), "8.4".parse().unwrap());

        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded);
        assert_eq!(
            log,
            vec![
                "unchanged: site acme".to_owned(),
                "done: 0 applied, 1 unchanged, 0 warning(s), 0 failed".to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn apply_warns_on_a_domain_owned_elsewhere() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let other = project(tmp.path(), "blog", None);
        link_site(&state, "blog", &other).await;
        ok(handle_mutation(
            Request::AddDomain {
                name: "blog".into(),
                domain: "shared.test".into(),
            },
            &state,
        )
        .await);
        let dir = project(tmp.path(), "acme", Some("domains = [\"shared\"]\n"));
        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded, "{log:?}");
        assert!(
            log.iter()
                .any(|l| l.starts_with(WARNING_PREFIX) && l.contains("shared.test")),
            "{log:?}"
        );
    }

    #[tokio::test]
    async fn apply_restores_a_declared_but_suppressed_apex() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let dir = project(tmp.path(), "acme", Some("domains = [\"acme\"]\n"));
        link_site(&state, "acme", &dir).await;
        ok(handle_mutation(add_domain("corp.acme.test"), &state).await);
        ok(handle_mutation(
            Request::RemoveDomain {
                name: "acme".into(),
                domain: "acme.test".into(),
            },
            &state,
        )
        .await);
        assert!(
            snapshot(&state, &site_of(&state, "acme").await)
                .await
                .apex_suppressed
        );
        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded, "{log:?}");
        assert!(
            log.contains(&"applied: domain acme.test".to_owned()),
            "{log:?}"
        );
        assert!(
            !snapshot(&state, &site_of(&state, "acme").await)
                .await
                .apex_suppressed
        );
    }

    #[tokio::test]
    async fn apply_fails_a_missing_legacy_php() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let dir = project(tmp.path(), "old", Some("php = \"7.4\"\n"));
        let (job, log, error) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Failed);
        assert_eq!(error.as_deref(), Some("1 item(s) could not be applied"));
        assert!(
            log.iter()
                .any(|l| l.starts_with(FAILED_PREFIX) && l.contains("--legacy")),
            "{log:?}"
        );
        assert!(state.router.read().await.get("old").is_some());
    }

    #[tokio::test]
    async fn apply_services_warn_on_mismatch_and_fail_offline() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let redis = yerd_services::version::server_path(
            &state.dirs,
            "redis",
            "valkey-server",
            &"7.2.4".parse().unwrap(),
        );
        std::fs::create_dir_all(redis.parent().unwrap()).unwrap();
        std::fs::write(&redis, b"#!fake").unwrap();
        let dir = project(
            tmp.path(),
            "acme",
            Some("[services]\nmysql = \"8.4\"\nredis = \"8\"\n"),
        );
        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Failed, "{log:?}");
        assert!(
            log.iter()
                .any(|l| l.starts_with(FAILED_PREFIX) && l.contains("mysql")),
            "{log:?}"
        );
        assert!(
            log.iter().any(|l| l.starts_with(WARNING_PREFIX)
                && l.contains("redis 7.2.4")
                && l.contains("change-version")),
            "{log:?}"
        );
    }

    #[tokio::test]
    async fn apply_services_judge_the_selected_version_not_any_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        for version in ["7.2.4", "8.0.1"] {
            let bin = yerd_services::version::server_path(
                &state.dirs,
                "redis",
                "valkey-server",
                &version.parse().unwrap(),
            );
            std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
            std::fs::write(&bin, b"#!fake").unwrap();
        }
        state.config.lock().await.services.instances.insert(
            "redis".to_owned(),
            yerd_config::ServiceInstance {
                version: Some("7.2.4".to_owned()),
                ..yerd_config::ServiceInstance::default()
            },
        );
        let dir = project(tmp.path(), "acme", Some("[services]\nredis = \"8\"\n"));
        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded, "{log:?}");
        assert!(
            log.iter().any(|l| l.starts_with(WARNING_PREFIX)
                && l.contains("redis 7.2.4 is installed")
                && l.contains("change-version")),
            "{log:?}"
        );
    }

    #[tokio::test]
    async fn apply_checks_extensions_without_changing_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        php_cli(&state, "8.3");
        let dir = project(
            tmp.path(),
            "acme",
            Some("extensions = [\"core\", \"redis\", \"xdebug\"]\n"),
        );
        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded, "{log:?}");
        assert!(
            log.contains(&"unchanged: extension core".to_owned()),
            "{log:?}"
        );
        assert!(
            log.iter()
                .any(|l| l.starts_with(WARNING_PREFIX) && l.contains("php ext add")),
            "{log:?}"
        );
        assert!(
            log.iter()
                .any(|l| l.starts_with(WARNING_PREFIX) && l.contains("yerd xdebug on 8.3")),
            "{log:?}"
        );
    }

    #[tokio::test]
    async fn start_apply_validates_synchronously() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let empty = project(tmp.path(), "empty", None);
        let resp = start_apply(empty.clone(), None, state.clone()).await;
        assert_eq!(code_of(&resp), ErrorCode::NotFound);

        let resp = start_apply(empty.clone(), Some("nope = 1".into()), state.clone()).await;
        assert_eq!(code_of(&resp), ErrorCode::ProjectFileInvalid);
        assert!(!empty.join(PROJECT_FILE_NAME).exists());

        let existing = project(tmp.path(), "existing", Some("secure = true\n"));
        let resp = start_apply(
            existing.clone(),
            Some("secure = false\n".into()),
            state.clone(),
        )
        .await;
        assert_eq!(code_of(&resp), ErrorCode::AlreadyExists);
        assert_eq!(
            std::fs::read_to_string(existing.join(PROJECT_FILE_NAME)).unwrap(),
            "secure = true\n"
        );
    }

    #[tokio::test]
    async fn start_apply_writes_the_wizard_contents_first() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let dir = project(tmp.path(), "fresh", None);
        let prepared = prepare(&dir, Some("name = \"shop\"\n".into()), &state)
            .await
            .unwrap();
        assert_eq!(prepared.name, "shop");
        let written = std::fs::read_to_string(prepared.dir.join(PROJECT_FILE_NAME)).unwrap();
        assert!(written.contains("name = \"shop\""), "{written}");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn sync_target_maps_each_synced_request() {
        let rule =
            |url: &str| ProxyRule::new("/app", UpstreamTarget::from_url_str(url).unwrap()).unwrap();
        let cases: Vec<(Request, Option<(&str, SyncChange)>)> = vec![
            (
                Request::SetPhp {
                    name: "Acme".into(),
                    version: "8.4".parse().unwrap(),
                },
                Some((
                    "acme",
                    SyncChange::Delta(SyncDelta::Php("8.4".parse().unwrap())),
                )),
            ),
            (
                Request::SetSecure {
                    name: "acme".into(),
                    secure: false,
                },
                Some(("acme", SyncChange::Delta(SyncDelta::Secure(false)))),
            ),
            (
                Request::AddDomain {
                    name: "acme".into(),
                    domain: "api.acme.test".into(),
                },
                Some(("acme", SyncChange::AddDomain("api.acme.test".into()))),
            ),
            (
                Request::RemoveDomain {
                    name: "acme".into(),
                    domain: "api.acme.test".into(),
                },
                Some(("acme", SyncChange::RemoveDomain("api.acme.test".into()))),
            ),
            (
                Request::ResetDomains {
                    name: "acme".into(),
                },
                Some(("acme", SyncChange::Delta(SyncDelta::ResetDomains))),
            ),
            (
                Request::SetResponseHeader {
                    site: "acme".into(),
                    name: "X-A".into(),
                    value: "1".into(),
                },
                Some((
                    "acme",
                    SyncChange::Delta(SyncDelta::SetHeader(
                        ResponseHeader::new("X-A", "1").unwrap(),
                    )),
                )),
            ),
            (
                Request::RemoveResponseHeader {
                    site: "acme".into(),
                    name: "X-A".into(),
                },
                Some((
                    "acme",
                    SyncChange::Delta(SyncDelta::RemoveHeader("X-A".into())),
                )),
            ),
            (
                Request::AddProxyRule {
                    site: "acme".into(),
                    prefix: "/app".into(),
                    url: "http://127.0.0.1:3000".into(),
                },
                Some((
                    "acme",
                    SyncChange::Delta(SyncDelta::SetRule(rule("http://127.0.0.1:3000"))),
                )),
            ),
            (
                Request::UpdateProxyRule {
                    site: "acme".into(),
                    prefix: "/app".into(),
                    url: "http://127.0.0.1:4000".into(),
                },
                Some((
                    "acme",
                    SyncChange::Delta(SyncDelta::SetRule(rule("http://127.0.0.1:4000"))),
                )),
            ),
            (
                Request::RemoveProxyRule {
                    site: "acme".into(),
                    prefix: "/app".into(),
                },
                Some((
                    "acme",
                    SyncChange::Delta(SyncDelta::RemoveRule("/app".into())),
                )),
            ),
            (
                Request::SetNodeVersion {
                    version: "22".into(),
                    site: Some("acme".into()),
                },
                Some((
                    "acme",
                    SyncChange::Delta(SyncDelta::Node(Some("22".into()))),
                )),
            ),
            (
                Request::ClearNodeVersion {
                    site: "acme".into(),
                },
                Some(("acme", SyncChange::Delta(SyncDelta::Node(None)))),
            ),
            (
                Request::Link {
                    name: "acme".into(),
                    path: PathBuf::from("/p"),
                },
                None,
            ),
            (
                Request::Unlink {
                    name: "acme".into(),
                },
                None,
            ),
            (
                Request::SetPrimaryDomain {
                    name: "acme".into(),
                    domain: "corp.acme.test".into(),
                },
                Some(("acme", SyncChange::AddDomain("corp.acme.test".into()))),
            ),
            (
                Request::SetNodeVersion {
                    version: "22".into(),
                    site: None,
                },
                None,
            ),
        ];
        for (req, want) in cases {
            let want = want.map(|(site, change)| (site.to_owned(), change));
            assert_eq!(sync_target(&req), want, "{req:?}");
        }
    }

    async fn synced_site(tmp: &Path, contents: Option<&str>) -> (Arc<DaemonState>, PathBuf) {
        let state = Arc::new(state_in(tmp));
        let dir = project(tmp, "acme", contents);
        link_site(&state, "acme", &dir).await;
        (state, dir.join(PROJECT_FILE_NAME))
    }

    async fn send(state: &Arc<DaemonState>, req: Request) {
        ok(crate::ipc_server::respond(req, state).await);
    }

    fn set_php(version: &str) -> Request {
        Request::SetPhp {
            name: "acme".into(),
            version: version.parse().unwrap(),
        }
    }

    fn add_domain(domain: &str) -> Request {
        Request::AddDomain {
            name: "acme".into(),
            domain: domain.into(),
        }
    }

    #[tokio::test]
    async fn sync_updates_only_the_changed_key() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, file) = synced_site(tmp.path(), Some("php = \"8.3\"\nsecure = true\n")).await;
        send(&state, set_php("8.4")).await;
        let parsed = ProjectFile::load(&file).unwrap().unwrap();
        assert_eq!(parsed.php, Some("8.4".parse().unwrap()));
        assert_eq!(parsed.secure, Some(true));
    }

    #[tokio::test]
    async fn sync_never_creates_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, file) = synced_site(tmp.path(), None).await;
        send(&state, set_php("8.4")).await;
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn sync_leaves_an_invalid_or_non_regular_file_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, file) = synced_site(tmp.path(), Some("colour = \"blue\"\n")).await;
        send(&state, set_php("8.4")).await;
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "colour = \"blue\"\n"
        );

        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        send(&state, set_php("8.2")).await;
        assert!(file.is_dir());
    }

    #[tokio::test]
    async fn sync_skips_the_apex_and_keeps_declared_domains() {
        let tmp = tempfile::tempdir().unwrap();
        let text = "domains = [\"declared.acme\"]\n";
        let (state, file) = synced_site(tmp.path(), Some(text)).await;
        send(&state, add_domain("acme.test")).await;
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);

        send(&state, add_domain("api.acme.test")).await;
        let parsed = ProjectFile::load(&file).unwrap().unwrap();
        let names: Vec<&str> = parsed.domains.iter().map(Domain::as_str).collect();
        assert_eq!(names, vec!["api.acme", "declared.acme"]);
    }

    #[tokio::test]
    async fn sync_adds_a_new_primary_domain_but_not_the_apex() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, file) = synced_site(tmp.path(), Some("")).await;
        for domain in ["corp.acme.test", "acme.test"] {
            send(
                &state,
                Request::SetPrimaryDomain {
                    name: "acme".into(),
                    domain: domain.into(),
                },
            )
            .await;
        }
        let parsed = ProjectFile::load(&file).unwrap().unwrap();
        let names: Vec<&str> = parsed.domains.iter().map(Domain::as_str).collect();
        assert_eq!(names, vec!["corp.acme"]);
    }

    #[tokio::test]
    async fn sync_writes_the_typed_node_selector() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, file) = synced_site(tmp.path(), Some("")).await;
        node_release(&state, "22.11.0");
        send(
            &state,
            Request::SetNodeVersion {
                version: "22".into(),
                site: Some("acme".into()),
            },
        )
        .await;
        let parsed = ProjectFile::load(&file).unwrap().unwrap();
        assert_eq!(parsed.node.as_deref(), Some("22"));
    }

    #[tokio::test]
    async fn sync_removes_only_the_named_header() {
        let tmp = tempfile::tempdir().unwrap();
        let text = "[headers]\nX-A = \"1\"\nX-B = \"2\"\n";
        let (state, file) = synced_site(tmp.path(), Some(text)).await;
        for name in ["X-A", "X-B"] {
            send(
                &state,
                Request::SetResponseHeader {
                    site: "acme".into(),
                    name: name.into(),
                    value: "1".into(),
                },
            )
            .await;
        }
        send(
            &state,
            Request::RemoveResponseHeader {
                site: "acme".into(),
                name: "x-a".into(),
            },
        )
        .await;
        let parsed = ProjectFile::load(&file).unwrap().unwrap();
        let names: Vec<&str> = parsed.headers.iter().map(ResponseHeader::name).collect();
        assert_eq!(names, vec!["X-B"]);
    }

    #[tokio::test]
    async fn sync_skips_an_unchanged_model() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, file) = synced_site(tmp.path(), Some("php = \"8.3\"\n")).await;
        send(&state, set_php("8.4")).await;
        let written = std::fs::read(&file).unwrap();
        let mtime = std::fs::metadata(&file).unwrap().modified().unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        send(&state, set_php("8.4")).await;
        assert_eq!(std::fs::read(&file).unwrap(), written);
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), mtime);
    }

    #[tokio::test]
    async fn sync_skips_a_parked_twin_of_a_linked_site() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let parent = tmp.path().join("Sites");
        let dir = project(&parent, "foo", Some("php = \"8.3\"\n"));
        ok(handle_mutation(Request::Park { path: parent }, &state).await);
        link_site(&state, "acme", &dir).await;
        let file = dir.join(PROJECT_FILE_NAME);
        send(
            &state,
            Request::SetPhp {
                name: "foo".into(),
                version: "8.4".parse().unwrap(),
            },
        )
        .await;
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "php = \"8.3\"\n");
        send(&state, set_php("8.2")).await;
        let parsed = ProjectFile::load(&file).unwrap().unwrap();
        assert_eq!(parsed.php, Some("8.2".parse().unwrap()));
    }

    #[tokio::test]
    async fn services_and_extensions_stop_when_cancelled() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        php_cli(&state, "8.3");
        let dir = project(tmp.path(), "acme", None);
        link_site(&state, "acme", &dir).await;
        let (job_id, cancel_rx) = state.jobs.create().await;
        ok(state.jobs.cancel(&job_id).await);
        let dl = UrlDl::default();
        let mut ap = Applier {
            id: &job_id,
            state: &state,
            dl: &dl,
            counts: Counts::default(),
        };
        let services: BTreeMap<String, String> = [("mysql".to_owned(), "8.4".to_owned())].into();
        assert!(!ap.services(&services, &cancel_rx).await);
        assert!(
            !ap.extensions("acme", &["core".to_owned()], &cancel_rx)
                .await
        );
        let Response::JobProgress { log, .. } = state.jobs.poll(&job_id, 0).await else {
            panic!("expected JobProgress");
        };
        assert!(log.is_empty(), "{log:?}");
    }

    #[tokio::test]
    async fn export_snapshots_after_waiting_for_the_file_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let dir = project(tmp.path(), "acme", None);
        link_site(&state, "acme", &dir).await;
        let guard = state.project_file_mutate.lock().await;
        let task = tokio::spawn({
            let state = state.clone();
            async move { export("acme", &state).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        ok(handle_mutation(
            Request::SetSecure {
                name: "acme".into(),
                secure: true,
            },
            &state,
        )
        .await);
        drop(guard);
        let (_, _, _, contents) = contents_of(task.await.unwrap());
        assert!(contents.contains("secure = true"), "{contents}");
    }

    #[tokio::test]
    async fn export_refuses_a_parked_twin_of_a_linked_site() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let parent = tmp.path().join("Sites");
        let dir = project(&parent, "foo", None);
        ok(handle_mutation(Request::Park { path: parent }, &state).await);
        link_site(&state, "acme", &dir).await;
        let resp = export("foo", &state).await;
        assert_eq!(code_of(&resp), ErrorCode::InvalidPath);
        let Response::Error { message, .. } = resp else {
            panic!("expected an error");
        };
        assert!(message.contains("acme"), "{message}");
        assert!(!dir.join(PROJECT_FILE_NAME).exists());
        let (_, site, _, _) = contents_of(export("acme", &state).await);
        assert_eq!(site.as_deref(), Some("acme"));
    }

    #[tokio::test]
    async fn apply_uses_the_file_prepare_validated() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let dir = project(tmp.path(), "checkout", Some("secure = true\n"));
        let prepared = prepare(&dir, None, &state).await.unwrap();
        std::fs::write(dir.join(PROJECT_FILE_NAME), "name = \"other\"\n").unwrap();
        let Response::JobStarted { job_id } = spawn_apply(prepared, state.clone(), offline()).await
        else {
            panic!("expected JobStarted");
        };
        let (job, log, _) = wait(&state, &job_id).await;
        assert_eq!(job, JobState::Succeeded, "{log:?}");
        assert!(site_of(&state, "checkout").await.secure());
        assert!(state.router.read().await.get("other").is_none());
    }

    #[tokio::test]
    async fn apply_cancelled_before_linking_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let dir = project(tmp.path(), "acme", Some("secure = true\n"));
        let prepared = prepare(&dir, None, &state).await.unwrap();
        let (job_id, cancel_rx) = state.jobs.create().await;
        ok(state.jobs.cancel(&job_id).await);
        let dl = UrlDl::default();
        let outcome = run_apply(
            &job_id,
            &prepared.dir,
            &prepared.file,
            &prepared.name,
            &state,
            &dl,
            &cancel_rx,
        )
        .await;
        assert!(matches!(outcome, Outcome::Cancelled));
        assert!(state.router.read().await.get("acme").is_none());
    }

    #[tokio::test]
    async fn apply_keeps_a_parked_manual_web_root() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let parent = tmp.path().join("Sites");
        let dir = project(&parent, "foo", Some("name = \"acme\"\n"));
        std::fs::create_dir_all(dir.join("web")).unwrap();
        ok(handle_mutation(Request::Park { path: parent }, &state).await);
        ok(handle_mutation(
            Request::SetWebRoot {
                name: "foo".into(),
                path: Some("web".into()),
            },
            &state,
        )
        .await);
        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded, "{log:?}");
        assert_eq!(
            site_of(&state, "acme").await.web_subpath(),
            Path::new("web")
        );
        assert!(
            log.contains(&"applied: web root web (kept from the parked site)".to_owned()),
            "{log:?}"
        );
    }

    #[tokio::test]
    async fn apply_leaves_auto_detect_without_a_parked_web_root() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let parent = tmp.path().join("Sites");
        let dir = project(&parent, "foo", Some("name = \"acme\"\n"));
        ok(handle_mutation(Request::Park { path: parent }, &state).await);
        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded, "{log:?}");
        assert!(!log.iter().any(|l| l.contains("web root")), "{log:?}");
    }

    struct CancellingDl(watch::Sender<bool>);

    #[async_trait::async_trait]
    impl Downloader for CancellingDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            self.0.send_replace(true);
            Err(yerd_php::DownloadError::Transport {
                url: url.to_owned(),
                reason: "offline".into(),
            })
        }
    }

    #[tokio::test]
    async fn services_stop_when_cancelled_during_the_listing_fetch() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let (job_id, _) = state.jobs.create().await;
        let (tx, cancel_rx) = watch::channel(false);
        let dl = CancellingDl(tx);
        let mut ap = Applier {
            id: &job_id,
            state: &state,
            dl: &dl,
            counts: Counts::default(),
        };
        let services: BTreeMap<String, String> = [("mysql".to_owned(), "8.4".to_owned())].into();
        assert!(!ap.services(&services, &cancel_rx).await);
        assert_eq!(ap.counts.failed, 0);
    }

    struct CancellingRunner(watch::Sender<bool>);

    #[async_trait::async_trait]
    impl yerd_php::CommandRunner for CancellingRunner {
        async fn run(&self, _cmd: std::process::Command) -> std::io::Result<yerd_php::ProbeOutput> {
            self.0.send_replace(true);
            Ok(yerd_php::ProbeOutput {
                status_ok: false,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn extensions_stop_when_cancelled_during_the_module_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, cancel_rx) = watch::channel(false);
        let mut state = state_in(tmp.path());
        state.ffi_cache = Arc::new(crate::ffi_cache::FfiCache::new(Arc::new(CancellingRunner(
            tx,
        ))));
        let state = Arc::new(state);
        php_cli(&state, "8.3");
        let dir = project(tmp.path(), "acme", None);
        link_site(&state, "acme", &dir).await;
        let (job_id, _) = state.jobs.create().await;
        let dl = UrlDl::default();
        let mut ap = Applier {
            id: &job_id,
            state: &state,
            dl: &dl,
            counts: Counts::default(),
        };
        assert!(
            !ap.extensions("acme", &["core".to_owned()], &cancel_rx)
                .await
        );
        assert_eq!(ap.counts.warnings, 0);
    }

    #[tokio::test]
    async fn sync_removes_a_declared_apex() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, file) = synced_site(tmp.path(), Some("domains = [\"acme\"]\n")).await;
        ok(handle_mutation(add_domain("corp.acme.test"), &state).await);
        send(
            &state,
            Request::RemoveDomain {
                name: "acme".into(),
                domain: "acme.test".into(),
            },
        )
        .await;
        let parsed = ProjectFile::load(&file).unwrap().unwrap();
        assert!(parsed.domains.is_empty(), "{:?}", parsed.domains);
    }

    #[test]
    fn an_existing_file_maps_to_already_exists() {
        let err = ProjectError::AlreadyExists {
            path: PathBuf::from("/p/.yerd.toml"),
        };
        assert_eq!(code_of(&project_error(&err)), ErrorCode::AlreadyExists);
    }

    /// 7.2.4 is not executable and 8.0.1 is, so the start error names which
    /// version was spawned without either one having to run.
    #[cfg(unix)]
    #[tokio::test]
    async fn apply_starts_the_installed_version_the_prefix_matched() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        for (version, mode) in [("7.2.4", 0o644), ("8.0.1", 0o755)] {
            let bin = yerd_services::version::server_path(
                &state.dirs,
                "redis",
                "valkey-server",
                &version.parse().unwrap(),
            );
            std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
            std::fs::write(&bin, b"#!/nonexistent/interpreter\n").unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        state.config.lock().await.services.instances.insert(
            "redis".to_owned(),
            yerd_config::ServiceInstance {
                port: Some(port),
                ..yerd_config::ServiceInstance::default()
            },
        );
        let dir = project(tmp.path(), "acme", Some("[services]\nredis = \"7\"\n"));
        let (_, log, _) = apply(&state, &dir).await;
        assert!(
            log.iter()
                .any(|l| l.starts_with("failed: start redis") && l.contains("PermissionDenied")),
            "{log:?}"
        );
    }

    #[test]
    fn same_node_selector_ignores_a_v_prefix_case_and_space() {
        let cases: &[(&str, &str, bool)] = &[
            ("v22", "22", true),
            ("V22.11", "22.11", true),
            (" 22\n", "22", true),
            ("lts/Iron", "lts/iron", true),
            ("20", "22", false),
            ("v22", "22.11", false),
        ];
        for (a, b, want) in cases {
            assert_eq!(same_node_selector(a, b), *want, "{a:?} {b:?}");
        }
    }

    #[tokio::test]
    async fn apply_does_not_warn_when_nvmrc_pins_the_same_node() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        node_release(&state, "22.11.0");
        let dir = project(tmp.path(), "acme", Some("node = \"22\"\n"));
        std::fs::write(dir.join(".nvmrc"), "v22\n").unwrap();
        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded, "{log:?}");
        assert!(
            !log.iter().any(|l| l.starts_with(WARNING_PREFIX)),
            "{log:?}"
        );

        std::fs::write(dir.join(".nvmrc"), "20\n").unwrap();
        let (_, log, _) = apply(&state, &dir).await;
        assert!(
            log.iter()
                .any(|l| l.starts_with(WARNING_PREFIX) && l.contains(".nvmrc pins Node 20")),
            "{log:?}"
        );
    }

    #[tokio::test]
    async fn apply_counts_a_bundled_pcov_as_available() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        php_cli(&state, "8.3");
        let dir = project(tmp.path(), "acme", Some("extensions = [\"pcov\"]\n"));
        let (_, log, _) = apply(&state, &dir).await;
        assert!(
            log.iter()
                .any(|l| l.starts_with(WARNING_PREFIX) && l.contains("extension pcov")),
            "{log:?}"
        );

        let so = crate::ext_install::pcov_so_path(&state.dirs, "8.3".parse().unwrap());
        std::fs::create_dir_all(so.parent().unwrap()).unwrap();
        std::fs::write(&so, b"so").unwrap();
        let (job, log, _) = apply(&state, &dir).await;
        assert_eq!(job, JobState::Succeeded, "{log:?}");
        assert!(
            log.iter()
                .any(|l| l.starts_with("unchanged: extension pcov") && l.contains("phpcover")),
            "{log:?}"
        );
    }

    #[tokio::test]
    async fn start_apply_rejects_a_domain_written_with_the_tld() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let fresh = project(tmp.path(), "fresh", None);
        let resp = start_apply(
            fresh.clone(),
            Some("domains = [\"admin.acme.test\"]\n".into()),
            state.clone(),
        )
        .await;
        assert_eq!(code_of(&resp), ErrorCode::ProjectFileInvalid);
        assert!(!fresh.join(PROJECT_FILE_NAME).exists());

        let existing = project(
            tmp.path(),
            "existing",
            Some("domains = [\"*.acme.test\"]\n"),
        );
        let resp = start_apply(existing, None, state.clone()).await;
        assert_eq!(code_of(&resp), ErrorCode::ProjectFileInvalid);
        assert!(state.router.read().await.get("existing").is_none());
    }

    #[tokio::test]
    async fn apply_reserves_the_site_already_at_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        let dir = project(tmp.path(), "shop", Some("name = \"acme\"\n"));
        link_site(&state, "shop", &dir).await;
        let prepared = prepare(&dir, None, &state).await.unwrap();
        assert_eq!(prepared.name, "acme");
        assert_eq!(prepared.owner.as_deref(), Some("shop"));
        state.reserved_names.lock().await.insert("shop".to_owned());
        let Response::JobStarted { job_id } = spawn_apply(prepared, state.clone(), offline()).await
        else {
            panic!("expected JobStarted");
        };
        let (job, _, error) = wait(&state, &job_id).await;
        assert_eq!(job, JobState::Failed);
        assert!(error.unwrap_or_default().contains("already being"));
        let reserved = state.reserved_names.lock().await.clone();
        assert_eq!(
            reserved,
            std::collections::HashSet::from(["shop".to_owned()])
        );
    }

    #[tokio::test]
    async fn reserve_all_is_all_or_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let names = |list: &[&str]| list.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        assert!(reserve_all(&state, &names(&["acme", "shop"])).await);
        assert!(!reserve_all(&state, &names(&["other", "shop"])).await);
        assert!(!state.reserved_names.lock().await.contains("other"));
        release_all(&state, &names(&["acme", "shop"])).await;
        assert!(state.reserved_names.lock().await.is_empty());
        assert!(reserve_all(&state, &names(&["other", "shop"])).await);
    }
}
