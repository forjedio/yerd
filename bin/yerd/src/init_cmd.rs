//! `yerd init`: apply the current directory's `.yerd.toml`, or create one with
//! a short wizard first.
//!
//! The daemon drafts, writes and applies the file; this module only asks the
//! questions (through the [`Prompter`] seam so the wizard is testable without a
//! terminal) and follows the apply job. Every answer is validated by the pure
//! `yerd_config::ProjectFile` rules before the file is sent.

use std::collections::BTreeMap;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Duration;

use yerd_config::{ProjectFile, ANY_SERVICE_VERSION, PROJECT_FILE_NAME};
use yerd_core::{Domain, PhpVersion};
use yerd_ipc::{JobState, Request, Response};

use crate::transport;

/// The answer that clears an optional, prefilled field.
pub const CLEAR_ANSWER: &str = "none";

/// Asks the wizard's questions. The real implementation reads stdin; tests
/// script the answers.
pub trait Prompter {
    /// Show `label` (with `default` when non-empty) and return the raw answer.
    /// An empty answer means "keep the default".
    ///
    /// # Errors
    /// An I/O error, including `UnexpectedEof` when input ends.
    fn ask(&mut self, label: &str, default: &str) -> io::Result<String>;

    /// Ask a yes/no question; an empty answer means `default`.
    ///
    /// # Errors
    /// An I/O error, including `UnexpectedEof` when input ends.
    fn confirm(&mut self, label: &str, default: bool) -> io::Result<bool>;

    /// Show a line of text (a note, a validation error or the preview).
    ///
    /// # Errors
    /// An I/O error writing the text.
    fn say(&mut self, text: &str) -> io::Result<()>;
}

/// A [`Prompter`] over the process's stdin and stdout.
pub struct StdPrompter;

impl StdPrompter {
    fn read_answer(prompt: &str) -> io::Result<String> {
        let mut out = io::stdout().lock();
        write!(out, "{prompt}")?;
        out.flush()?;
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        Ok(line.trim().to_owned())
    }
}

impl Prompter for StdPrompter {
    fn ask(&mut self, label: &str, default: &str) -> io::Result<String> {
        if default.is_empty() {
            Self::read_answer(&format!("{label}: "))
        } else {
            Self::read_answer(&format!("{label} [{default}]: "))
        }
    }

    fn confirm(&mut self, label: &str, default: bool) -> io::Result<bool> {
        let hint = if default { "Y/n" } else { "y/N" };
        loop {
            let answer = Self::read_answer(&format!("{label} [{hint}]: "))?;
            match answer.to_ascii_lowercase().as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => self.say("  please answer y or n")?,
            }
        }
    }

    fn say(&mut self, text: &str) -> io::Result<()> {
        writeln!(io::stdout().lock(), "{text}")
    }
}

/// Run `yerd init` in the current directory and return the process exit code:
/// `0` success, `1` a failed or aborted apply, `2` usage (no file and no
/// terminal, or `--json` without a file), `69` daemon down, `74` transport.
pub async fn run(json: bool) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            return fail(
                json,
                &[],
                &format!("cannot resolve the current directory: {e}"),
                2,
            )
        }
    };
    let reply = transport::exchange(&Request::ProjectConfigDraft { path: cwd.clone() }).await;
    let (file, exists, contents) = match draft_reply(reply) {
        Ok(draft) => draft,
        Err((message, code)) => return fail(json, &[], &message, code),
    };
    if exists {
        if !json {
            println!("Applying {}", file.display());
        }
        return follow(
            Request::ApplyProjectConfig {
                path: cwd,
                contents: None,
            },
            json,
        )
        .await;
    }
    if json || !io::stdin().is_terminal() {
        return fail(
            json,
            &[],
            &format!("no {PROJECT_FILE_NAME} here - run `yerd init` in a terminal to create one"),
            2,
        );
    }
    let draft = match ProjectFile::from_toml(&contents) {
        Ok(draft) => draft,
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(1);
        }
    };
    let note = node_pin_note(&cwd);
    let tld = daemon_tld().await;
    match wizard(&mut StdPrompter, draft, note.as_deref(), tld.as_deref()) {
        Ok(Some(request)) => follow(request.into_request(cwd), json).await,
        Ok(None) => {
            println!("Nothing written.");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("\nyerd: aborted ({e})");
            ExitCode::from(1)
        }
    }
}

/// The draft's `(file, exists, contents)`, or the failure message and exit
/// code for a draft request that failed: `1` for a daemon error reply, `69`
/// daemon down, `74` an unexpected reply or other transport error.
///
/// # Errors
/// The message and exit code to report.
pub fn draft_reply(
    reply: Result<Response, crate::ClientError>,
) -> Result<(PathBuf, bool, String), (String, u8)> {
    match reply {
        Ok(Response::ProjectConfig {
            file,
            exists,
            contents,
            ..
        }) => Ok((file, exists, contents)),
        Ok(Response::Error { message, .. }) => Err((message, 1)),
        Ok(_) => Err((
            format!("unexpected response reading {PROJECT_FILE_NAME}"),
            74,
        )),
        Err(e) => Err((e.to_string(), transport_code(&e))),
    }
}

async fn daemon_tld() -> Option<String> {
    match transport::exchange(&Request::DaemonInfo).await {
        Ok(Response::Info { tld, .. }) => Some(tld),
        _ => None,
    }
}

fn node_pin_note(dir: &Path) -> Option<String> {
    let pinned = yerd_platform::node::project_file(dir, Some(dir)).ok()??;
    Some(format!(
        "{} pins Node {}; it wins over a node version set here.",
        pinned.kind.file_name(),
        pinned.selector
    ))
}

/// The wizard's result: the file to write and apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardResult {
    /// The rendered `.yerd.toml` text.
    pub contents: String,
}

impl WizardResult {
    /// The `ApplyProjectConfig` request that writes and applies the file in `dir`.
    #[must_use]
    pub fn into_request(self, dir: PathBuf) -> Request {
        Request::ApplyProjectConfig {
            path: dir,
            contents: Some(self.contents),
        }
    }
}

/// Ask the wizard's questions over `draft` (headers and proxy rules carry over
/// unprompted), show the rendered file, and confirm. `Ok(None)` when the user
/// declines. `node_note` is shown before the Node question. With the daemon's
/// `tld` known, an extra domain that already ends in `.<tld>` is asked again.
///
/// # Errors
/// The prompter's I/O error, including `UnexpectedEof` when input ends.
pub fn wizard(
    p: &mut dyn Prompter,
    draft: ProjectFile,
    node_note: Option<&str>,
    tld: Option<&str>,
) -> io::Result<Option<WizardResult>> {
    let mut file = draft;
    let name = ask_until(p, "Site name", file.name.as_deref().unwrap_or(""), |a| {
        yerd_core::normalize_site_name(a).ok_or_else(|| format!("{a:?} is not a valid site name"))
    })?;
    file.name = Some(name);
    let php_default = file.php.map(|v| v.to_string()).unwrap_or_default();
    let php = ask_until(p, "PHP version", &php_default, |a| {
        PhpVersion::from_str(a).map_err(|e| e.to_string())
    })?;
    file.php = Some(php);
    file.secure = Some(p.confirm("Serve over HTTPS?", file.secure.unwrap_or(false))?);
    if let Some(note) = node_note {
        p.say(note)?;
    }
    let node_label = format!("Node version (blank = not managed, `{CLEAR_ANSWER}` clears)");
    file.node = ask_until(p, &node_label, file.node.as_deref().unwrap_or(""), |a| {
        optional(a).map_or(Ok(None), |sel| {
            yerd_core::node::validate_selector(sel).map(|()| Some(sel.to_owned()))
        })
    })?;
    let services_default = render_services(&file.services);
    file.services = ask_until(
        p,
        "Services (comma list of engine[@version], e.g. mysql@8.4, redis)",
        &services_default,
        |a| {
            let services = parse_services(a)?;
            check(ProjectFile {
                services: services.clone(),
                ..ProjectFile::default()
            })?;
            Ok(services)
        },
    )?;
    let domains_default = join(file.domains.iter().map(Domain::as_str));
    file.domains = ask_until(
        p,
        "Extra domains without the TLD (comma list, e.g. admin.acme, *.acme)",
        &domains_default,
        |a| {
            let mut domains = list(a)
                .map(|d| extra_domain(d, tld))
                .collect::<Result<Vec<_>, _>>()?;
            domains.sort();
            domains.dedup();
            Ok(domains)
        },
    )?;
    let extensions_default = join(file.extensions.iter().map(String::as_str));
    file.extensions = ask_until(
        p,
        "Required PHP extensions (comma list, e.g. redis, imagick)",
        &extensions_default,
        |a| {
            let mut extensions: Vec<String> = list(a).map(str::to_ascii_lowercase).collect();
            extensions.sort();
            extensions.dedup();
            check(ProjectFile {
                extensions: extensions.clone(),
                ..ProjectFile::default()
            })?;
            Ok(extensions)
        },
    )?;
    if let Err(e) = file.validate() {
        p.say(&format!("  {e}"))?;
        return Ok(None);
    }
    let contents = file.to_toml();
    p.say("")?;
    p.say(&contents)?;
    if p.confirm(&format!("Write {PROJECT_FILE_NAME} and apply it?"), true)? {
        Ok(Some(WizardResult { contents }))
    } else {
        Ok(None)
    }
}

fn extra_domain(answer: &str, tld: Option<&str>) -> Result<Domain, String> {
    let domain = Domain::parse_subpart(answer).map_err(|e| e.to_string())?;
    match tld {
        Some(tld) if yerd_config::ends_with_tld(&domain, tld) => Err(format!(
            "{answer} already ends in .{tld}; enter it without the TLD"
        )),
        _ => Ok(domain),
    }
}

fn ask_until<T>(
    p: &mut dyn Prompter,
    label: &str,
    default: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> io::Result<T> {
    loop {
        let answer = p.ask(label, default)?;
        let answer = answer.trim();
        let effective = if answer.is_empty() { default } else { answer };
        match parse(effective) {
            Ok(value) => return Ok(value),
            Err(e) => p.say(&format!("  {e}"))?,
        }
    }
}

fn optional(answer: &str) -> Option<&str> {
    let answer = answer.trim();
    (!answer.is_empty() && !answer.eq_ignore_ascii_case(CLEAR_ANSWER)).then_some(answer)
}

fn list(answer: &str) -> impl Iterator<Item = &str> {
    optional(answer)
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn join<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items.collect::<Vec<_>>().join(", ")
}

fn check(file: ProjectFile) -> Result<(), String> {
    file.validate().map_err(|e| e.to_string())
}

fn render_services(services: &BTreeMap<String, String>) -> String {
    services
        .iter()
        .map(|(engine, version)| {
            if version == ANY_SERVICE_VERSION {
                engine.clone()
            } else {
                format!("{engine}@{version}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_services(answer: &str) -> Result<BTreeMap<String, String>, String> {
    let mut services = BTreeMap::new();
    for item in list(answer) {
        let (engine, version) = item.split_once('@').unwrap_or((item, ANY_SERVICE_VERSION));
        let engine = engine.trim().to_ascii_lowercase();
        let version = version.trim();
        if services
            .insert(engine.clone(), version.to_owned())
            .is_some()
        {
            return Err(format!("{engine} is listed twice"));
        }
    }
    Ok(services)
}

fn transport_code(e: &crate::ClientError) -> u8 {
    if e.is_daemon_down() {
        69
    } else {
        74
    }
}

/// What a failed start or poll of the apply job prints: the stdout line (the
/// one `--json` object, `None` in human mode) and the stderr line.
#[must_use]
pub fn failure_output(json: bool, collected: &[String], message: &str) -> (Option<String>, String) {
    (
        json.then(|| summary_json(JobState::Failed, collected, Some(message))),
        format!("yerd: {message}"),
    )
}

fn fail(json: bool, collected: &[String], message: &str, code: u8) -> ExitCode {
    let (stdout, stderr) = failure_output(json, collected, message);
    if let Some(line) = stdout {
        println!("{line}");
    }
    eprintln!("{stderr}");
    ExitCode::from(code)
}

/// Start the apply job with `start` and follow it to the end. Human mode
/// prints each outcome line as it arrives; `--json` prints one object
/// `{ "state", "log", "error" }` once the job ends, or when starting or
/// polling it fails.
async fn follow(start: Request, json: bool) -> ExitCode {
    let job_id = match transport::exchange(&start).await {
        Ok(Response::JobStarted { job_id }) => job_id,
        Ok(Response::Error { message, .. }) => return fail(json, &[], &message, 1),
        Ok(_) => return fail(json, &[], "unexpected response starting the apply", 74),
        Err(e) => return fail(json, &[], &e.to_string(), transport_code(&e)),
    };
    let mut cursor = 0u64;
    let mut collected: Vec<String> = Vec::new();
    loop {
        let poll = Request::JobStatus {
            job_id: job_id.clone(),
            cursor,
        };
        let (state, log, next_cursor, error) = match transport::exchange(&poll).await {
            Ok(Response::JobProgress {
                state,
                log,
                next_cursor,
                error,
                ..
            }) => (state, log, next_cursor, error),
            Ok(Response::Error { message, .. }) => return fail(json, &collected, &message, 1),
            Ok(_) => {
                return fail(
                    json,
                    &collected,
                    "unexpected response polling the apply",
                    74,
                )
            }
            Err(e) => return fail(json, &collected, &e.to_string(), transport_code(&e)),
        };
        cursor = next_cursor;
        if json {
            collected.extend(log);
        } else {
            for line in &log {
                println!("{line}");
            }
        }
        if state == JobState::Running {
            tokio::time::sleep(Duration::from_millis(400)).await;
            continue;
        }
        if json {
            println!("{}", summary_json(state, &collected, error.as_deref()));
        } else if let Some(e) = &error {
            eprintln!("yerd: {e}");
        } else if state == JobState::Cancelled {
            eprintln!("yerd: apply cancelled");
        }
        return if state == JobState::Succeeded {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        };
    }
}

/// The `--json` summary of a finished apply job.
#[must_use]
pub fn summary_json(state: JobState, log: &[String], error: Option<&str>) -> String {
    serde_json::json!({ "state": state, "log": log, "error": error }).to_string()
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
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Scripted {
        answers: VecDeque<&'static str>,
        asked: Vec<String>,
        said: Vec<String>,
    }

    impl Scripted {
        fn new(answers: &[&'static str]) -> Self {
            Self {
                answers: answers.iter().copied().collect(),
                ..Self::default()
            }
        }

        fn next(&mut self) -> io::Result<&'static str> {
            self.answers
                .pop_front()
                .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))
        }
    }

    impl Prompter for Scripted {
        fn ask(&mut self, label: &str, _default: &str) -> io::Result<String> {
            self.asked.push(label.to_owned());
            self.next().map(str::to_owned)
        }

        fn confirm(&mut self, label: &str, default: bool) -> io::Result<bool> {
            self.asked.push(label.to_owned());
            Ok(match self.next()? {
                "" => default,
                answer => answer == "y",
            })
        }

        fn say(&mut self, text: &str) -> io::Result<()> {
            self.said.push(text.to_owned());
            Ok(())
        }
    }

    fn draft() -> ProjectFile {
        ProjectFile::from_toml(
            "name = \"acme\"\nphp = \"8.3\"\nsecure = false\n\n[headers]\nX-A = \"1\"\n",
        )
        .unwrap()
    }

    fn run_wizard(answers: &[&'static str], note: Option<&str>) -> (Option<String>, Scripted) {
        let mut p = Scripted::new(answers);
        let out = wizard(&mut p, draft(), note, None).unwrap();
        (out.map(|r| r.contents), p)
    }

    #[test]
    fn accepting_every_default_renders_the_draft() {
        let (contents, _) = run_wizard(&["", "", "", "", "", "", "", ""], None);
        assert_eq!(contents.unwrap(), draft().to_toml());
    }

    #[test]
    fn invalid_answers_are_asked_again() {
        let (contents, p) = run_wizard(&["", "banana", "8.4", "", "", "", "", "", ""], None);
        let contents = contents.unwrap();
        assert!(contents.contains("php = \"8.4\""), "{contents}");
        assert_eq!(p.asked.iter().filter(|l| *l == "PHP version").count(), 2);
        assert!(p.said.iter().any(|s| s.contains("banana")), "{:?}", p.said);
    }

    #[test]
    fn answers_build_the_file() {
        let (contents, p) = run_wizard(
            &[
                "Shop",
                "8.4",
                "y",
                "22",
                "mysql@8.4, redis",
                "admin.shop, *.shop",
                "Redis",
                "y",
            ],
            Some("an .nvmrc pins Node 20"),
        );
        let file = ProjectFile::from_toml(&contents.unwrap()).unwrap();
        assert_eq!(file.name.as_deref(), Some("shop"));
        assert_eq!(file.secure, Some(true));
        assert_eq!(file.node.as_deref(), Some("22"));
        assert_eq!(file.services["mysql"], "8.4");
        assert_eq!(file.services["redis"], "*");
        assert_eq!(file.domains.len(), 2);
        assert_eq!(file.extensions, vec!["redis".to_owned()]);
        assert_eq!(file.headers.len(), 1);
        assert!(p.said.contains(&"an .nvmrc pins Node 20".to_owned()));
    }

    #[test]
    fn blank_node_omits_the_key_and_none_clears_it() {
        let (contents, _) = run_wizard(&["", "", "", "", "", "", "", ""], None);
        assert!(!contents.unwrap().contains("node ="));

        let mut p = Scripted::new(&["", "", "", "none", "", "", "", ""]);
        let mut with_node = draft();
        with_node.node = Some("22".to_owned());
        let contents = wizard(&mut p, with_node, None, None)
            .unwrap()
            .unwrap()
            .contents;
        assert!(!contents.contains("node ="), "{contents}");
    }

    #[test]
    fn unknown_service_is_asked_again() {
        let (contents, p) = run_wizard(&["", "", "", "", "mongodb", "redis@7", "", "", ""], None);
        assert!(contents.unwrap().contains("redis = \"7\""));
        assert_eq!(
            p.asked.iter().filter(|l| l.starts_with("Services")).count(),
            2
        );
    }

    #[test]
    fn declining_builds_no_request() {
        let (contents, _) = run_wizard(&["", "", "", "", "", "", "", "n"], None);
        assert_eq!(contents, None);
    }

    #[test]
    fn end_of_input_aborts() {
        let mut p = Scripted::new(&["", ""]);
        let err = wizard(&mut p, draft(), None, None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn service_lists_round_trip() {
        let cases: &[(&str, &[(&str, &str)])] = &[
            ("", &[]),
            ("none", &[]),
            ("mysql@8.4", &[("mysql", "8.4")]),
            (
                " Redis , postgres@17 ",
                &[("postgres", "17"), ("redis", "*")],
            ),
        ];
        for (input, want) in cases {
            let got = parse_services(input).unwrap();
            let want: BTreeMap<String, String> = want
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            assert_eq!(got, want, "{input:?}");
        }
        assert!(parse_services("redis, redis@7").is_err());
        let services = parse_services("mysql@8.4, redis").unwrap();
        assert_eq!(render_services(&services), "mysql@8.4, redis");
    }

    #[test]
    fn wizard_result_maps_to_an_apply_request() {
        let req = WizardResult {
            contents: "php = \"8.4\"\n".to_owned(),
        }
        .into_request(PathBuf::from("/p/acme"));
        assert_eq!(
            req,
            Request::ApplyProjectConfig {
                path: PathBuf::from("/p/acme"),
                contents: Some("php = \"8.4\"\n".to_owned()),
            }
        );
    }

    #[test]
    fn failure_output_prints_one_json_object_only_in_json_mode() {
        let collected = vec!["applied: linked acme".to_owned()];
        let cases: &[(bool, &[String], Option<&str>)] = &[
            (false, &collected, None),
            (
                true,
                &[],
                Some(r#"{"error":"boom","log":[],"state":"failed"}"#),
            ),
            (
                true,
                &collected,
                Some(r#"{"error":"boom","log":["applied: linked acme"],"state":"failed"}"#),
            ),
        ];
        for (json, log, want) in cases {
            let (stdout, stderr) = failure_output(*json, log, "boom");
            assert_eq!(stderr, "yerd: boom");
            let got = stdout.map(|s| serde_json::from_str::<serde_json::Value>(&s).unwrap());
            let want = want.map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap());
            assert_eq!(got, want, "json={json}");
        }
    }

    #[test]
    fn summary_json_shape() {
        let json = summary_json(JobState::Failed, &["failed: x".to_owned()], Some("1 item"));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["state"], "failed");
        assert_eq!(value["log"][0], "failed: x");
        assert_eq!(value["error"], "1 item");
    }

    #[test]
    fn extra_domains_with_the_tld_are_asked_again() {
        let mut p = Scripted::new(&["", "", "", "", "", "admin.acme.test", "admin.acme", "", ""]);
        let contents = wizard(&mut p, draft(), None, Some("test"))
            .unwrap()
            .unwrap()
            .contents;
        assert!(
            contents.contains("domains = [\"admin.acme\"]"),
            "{contents}"
        );
        assert_eq!(
            p.asked
                .iter()
                .filter(|l| l.starts_with("Extra domains"))
                .count(),
            2
        );
        assert!(
            p.said.iter().any(|s| s.contains("already ends in .test")),
            "{:?}",
            p.said
        );
    }

    #[test]
    fn draft_reply_maps_each_failure_to_a_message_and_exit_code() {
        let ok = draft_reply(Ok(Response::ProjectConfig {
            file: PathBuf::from("/p/.yerd.toml"),
            exists: true,
            site: None,
            name: None,
            contents: "php = \"8.4\"\n".to_owned(),
        }));
        assert_eq!(
            ok,
            Ok((
                PathBuf::from("/p/.yerd.toml"),
                true,
                "php = \"8.4\"\n".to_owned()
            ))
        );
        let cases: Vec<(Result<Response, crate::ClientError>, &str, u8)> = vec![
            (
                Ok(Response::Error {
                    code: yerd_ipc::ErrorCode::ProjectFileInvalid,
                    message: "bad key".to_owned(),
                }),
                "bad key",
                1,
            ),
            (
                Ok(Response::Error {
                    code: yerd_ipc::ErrorCode::NotFound,
                    message: "no such dir".to_owned(),
                }),
                "no such dir",
                1,
            ),
            (Ok(Response::Ok), "unexpected response", 74),
            (
                Err(crate::ClientError::DaemonUnreachable("refused".to_owned())),
                "daemon not running",
                69,
            ),
            (Err(crate::ClientError::Usage("odd".to_owned())), "odd", 74),
        ];
        for (reply, want_message, want_code) in cases {
            let (message, code) = draft_reply(reply).unwrap_err();
            assert!(message.contains(want_message), "{message}");
            assert_eq!(code, want_code, "{message}");
            let (stdout, _) = failure_output(true, &[], &message);
            let value: serde_json::Value = serde_json::from_str(&stdout.unwrap()).unwrap();
            assert_eq!(value["state"], "failed");
            assert_eq!(value["error"], message.as_str());
        }
    }
}
