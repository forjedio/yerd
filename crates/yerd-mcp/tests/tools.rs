//! The tool catalog: every tool maps to the daemon request it claims to, and
//! every catalog entry is well-formed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};
use yerd_core::PhpVersion;
use yerd_ipc::Request;
use yerd_mcp::{Availability, Outgoing, Server, LATEST_PROTOCOL_VERSION};

fn ready() -> Server {
    let mut s = Server::new(Availability::Enabled, "9.9.9");
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": LATEST_PROTOCOL_VERSION, "capabilities": {} },
    })
    .to_string();
    let _ = s.handle_line(&init);
    s
}

/// Drive one `tools/call` and return the request it produced.
fn built(tool: &str, args: Value) -> Request {
    let line = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    })
    .to_string();
    match ready().handle_line(&line) {
        Outgoing::CallDaemon(call) => call.request,
        other => panic!("{tool} did not produce a daemon call: {other:?}"),
    }
}

fn tools_list() -> Vec<Value> {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string();
    let Outgoing::Reply(reply) = ready().handle_line(&line) else {
        panic!("tools/list did not reply");
    };
    let v: Value = serde_json::from_str(&reply).unwrap();
    v.pointer("/result/tools")
        .and_then(Value::as_array)
        .cloned()
        .expect("tools array")
}

fn tool_names() -> Vec<String> {
    tools_list()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect()
}

/// The catalog is the contract agents build their calls against, so adding,
/// removing, or renaming a tool should be a deliberate, reviewed act rather
/// than a side effect of another change.
#[test]
fn catalog_names_are_pinned() {
    let expected = [
        "list_sites",
        "create_site",
        "link_site",
        "park_directory",
        "list_parked",
        "set_site_php",
        "set_site_secure",
        "add_domain",
        "remove_domain",
        "add_proxy",
        "remove_proxy",
        "add_proxy_rule",
        "remove_proxy_rule",
        "list_proxies",
        "list_php",
        "list_available_php",
        "install_php",
        "set_default_php",
        "set_php_setting",
        "list_services",
        "list_databases",
        "create_database",
        "set_mail_enabled",
        "list_mails",
        "get_mail",
        "set_dumps_enabled",
        "dumps_status",
        "list_dumps",
        "status",
        "diagnose",
        "job_status",
    ];
    assert_eq!(tool_names(), expected);
}

/// v1 deliberately excludes anything that destroys data or uninstalls software:
/// an agent can create and configure, not demolish. This exclusion is the
/// feature's safety story, so it is pinned rather than left to review.
#[test]
fn no_destructive_tools_are_exposed() {
    let banned = [
        "drop_database",
        "uninstall_php",
        "uninstall_service",
        "unlink_site",
        "unpark_directory",
        "clear_mails",
        "delete_mail",
        "clear_dumps",
        "delete_dump",
        "stop_service",
        "doctor_fix",
        "job_cancel",
        "restart_daemon",
    ];
    for name in tool_names() {
        assert!(
            !banned.contains(&name.as_str()),
            "{name} must not be exposed"
        );
    }
}

/// Note `inputSchema` is asserted by its exact camelCase name, per the MCP
/// spec: a `snake_case` key is silently ignored by clients, which then see a
/// schema-less tool rather than an error.
#[test]
fn every_tool_entry_is_well_formed() {
    for tool in tools_list() {
        let name = tool["name"].as_str().expect("name is a string");
        let description = tool["description"].as_str().expect("description present");
        assert!(!description.is_empty(), "{name} has no description");
        assert!(
            description.ends_with('.'),
            "{name}: description should read as a sentence"
        );
        let schema = tool
            .get("inputSchema")
            .unwrap_or_else(|| panic!("{name} has no inputSchema"));
        assert_eq!(schema["type"], json!("object"), "{name} schema type");
        assert!(schema.get("properties").is_some(), "{name} has properties");
        assert_eq!(
            tool.as_object().map(serde_json::Map::len),
            Some(3),
            "{name} should carry exactly name/description/inputSchema"
        );
    }
}

#[test]
fn required_arguments_are_declared_in_the_schema() {
    let expected: BTreeMap<&str, Vec<&str>> = [
        ("create_site", vec!["name", "parent_dir", "php"]),
        ("link_site", vec!["name", "path"]),
        ("park_directory", vec!["path"]),
        ("set_site_php", vec!["name", "version"]),
        ("set_site_secure", vec!["name", "secure"]),
        ("add_domain", vec!["name", "domain"]),
        ("remove_domain", vec!["name", "domain"]),
        ("add_proxy", vec!["name", "url"]),
        ("remove_proxy", vec!["name"]),
        ("add_proxy_rule", vec!["site", "prefix", "url"]),
        ("remove_proxy_rule", vec!["site", "prefix"]),
        ("install_php", vec!["version"]),
        ("set_default_php", vec!["version"]),
        ("set_php_setting", vec!["name", "value"]),
        ("list_databases", vec!["service"]),
        ("create_database", vec!["service", "name"]),
        ("set_mail_enabled", vec!["enabled"]),
        ("get_mail", vec!["id"]),
        ("set_dumps_enabled", vec!["enabled"]),
        ("job_status", vec!["job_id"]),
    ]
    .into_iter()
    .collect();

    for tool in tools_list() {
        let name = tool["name"].as_str().unwrap();
        let required: Vec<&str> = tool
            .pointer("/inputSchema/required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            required,
            expected.get(name).cloned().unwrap_or_default(),
            "{name} required args"
        );
    }
}

/// The advertised schema and the builder must name arguments identically.
///
/// Nothing else pins this, and a divergence fails *silently* in the worst
/// direction: optional arguments default rather than error, so an agent that
/// obeys the published schema would have its value dropped without a word. A
/// renamed `since_id`, for instance, would silently page the entire dump buffer
/// on every call.
#[test]
fn schema_property_names_match_the_builder() {
    let expected: BTreeMap<&str, Vec<&str>> = [
        (
            "create_site",
            vec![
                "database",
                "git",
                "js",
                "name",
                "parent_dir",
                "php",
                "secure",
                "starter_kit",
                "testing",
            ],
        ),
        ("link_site", vec!["name", "path"]),
        ("park_directory", vec!["path"]),
        ("set_site_php", vec!["name", "version"]),
        ("set_site_secure", vec!["name", "secure"]),
        ("add_domain", vec!["domain", "name"]),
        ("remove_domain", vec!["domain", "name"]),
        ("add_proxy", vec!["name", "url"]),
        ("remove_proxy", vec!["name"]),
        ("add_proxy_rule", vec!["prefix", "site", "url"]),
        ("remove_proxy_rule", vec!["prefix", "site"]),
        ("install_php", vec!["version"]),
        ("set_default_php", vec!["version"]),
        ("set_php_setting", vec!["name", "value"]),
        ("list_databases", vec!["service"]),
        ("create_database", vec!["name", "service"]),
        ("set_mail_enabled", vec!["enabled"]),
        ("get_mail", vec!["id"]),
        ("set_dumps_enabled", vec!["enabled"]),
        ("list_dumps", vec!["since_id"]),
        ("job_status", vec!["cursor", "job_id"]),
    ]
    .into_iter()
    .collect();

    for tool in tools_list() {
        let name = tool["name"].as_str().unwrap();
        let properties: Vec<&str> = tool
            .pointer("/inputSchema/properties")
            .and_then(Value::as_object)
            .map(|o| o.keys().map(String::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            properties,
            expected.get(name).cloned().unwrap_or_default(),
            "{name}'s schema properties"
        );

        let required: Vec<&str> = tool
            .pointer("/inputSchema/required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for arg in required {
            assert!(
                properties.contains(&arg),
                "{name} requires `{arg}` but never declares it"
            );
        }
    }
}

/// Every optional argument, when supplied, must actually change the request the
/// daemon receives. A property the builder does not read would otherwise sit in
/// the schema looking functional while being silently ignored.
#[test]
fn optional_arguments_reach_the_request() {
    let base = json!({ "name": "app", "parent_dir": "/srv", "php": "8.4" });
    let with = |key: &str, value: Value| {
        let mut args = base.clone();
        args[key] = value;
        built("create_site", args)
    };
    let default = built("create_site", base.clone());

    for (key, value) in [
        ("secure", json!(false)),
        ("starter_kit", json!("react")),
        ("testing", json!("phpunit")),
        ("database", json!("mysql")),
        ("js", json!("bun")),
        ("git", json!(true)),
    ] {
        assert_ne!(
            with(key, value.clone()),
            default,
            "create_site ignored `{key}` = {value}"
        );
    }

    assert_ne!(
        built("list_dumps", json!({ "since_id": 99 })),
        built("list_dumps", json!({})),
        "list_dumps ignored `since_id`"
    );
    assert_ne!(
        built("job_status", json!({ "job_id": "j", "cursor": 5 })),
        built("job_status", json!({ "job_id": "j" })),
        "job_status ignored `cursor`"
    );
}

/// Guards the catalog against an entry with no builder arm. `build` resolves the
/// name against the table first, so a missing arm is not a compile error - it
/// would surface as a mystery `unknown tool` at runtime, for a tool the server
/// itself advertised.
#[test]
fn every_tool_builds_a_request() {
    let args: BTreeMap<&str, Value> = [
        (
            "create_site",
            json!({ "name": "app", "parent_dir": "/srv", "php": "8.4" }),
        ),
        ("link_site", json!({ "name": "app", "path": "/srv/app" })),
        ("park_directory", json!({ "path": "/srv" })),
        ("set_site_php", json!({ "name": "app", "version": "8.4" })),
        ("set_site_secure", json!({ "name": "app", "secure": true })),
        ("add_domain", json!({ "name": "app", "domain": "a.test" })),
        (
            "remove_domain",
            json!({ "name": "app", "domain": "a.test" }),
        ),
        (
            "add_proxy",
            json!({ "name": "rev", "url": "http://127.0.0.1:9000" }),
        ),
        ("remove_proxy", json!({ "name": "rev" })),
        (
            "add_proxy_rule",
            json!({ "site": "app", "prefix": "/r", "url": "http://127.0.0.1:9000" }),
        ),
        (
            "remove_proxy_rule",
            json!({ "site": "app", "prefix": "/r" }),
        ),
        ("install_php", json!({ "version": "8.4" })),
        ("set_default_php", json!({ "version": "8.4" })),
        (
            "set_php_setting",
            json!({ "name": "memory_limit", "value": "512M" }),
        ),
        ("list_databases", json!({ "service": "mysql" })),
        (
            "create_database",
            json!({ "service": "mysql", "name": "db" }),
        ),
        ("set_mail_enabled", json!({ "enabled": true })),
        ("get_mail", json!({ "id": "000001" })),
        ("set_dumps_enabled", json!({ "enabled": true })),
        ("job_status", json!({ "job_id": "j1" })),
    ]
    .into_iter()
    .collect();

    for name in tool_names() {
        let a = args
            .get(name.as_str())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let _ = built(&name, a);
    }
}

#[test]
fn read_tools_map_to_their_requests() {
    assert_eq!(built("list_sites", json!({})), Request::ListSites);
    assert_eq!(built("list_parked", json!({})), Request::ListParked);
    assert_eq!(built("list_proxies", json!({})), Request::ListProxies);
    assert_eq!(built("list_php", json!({})), Request::ListPhp);
    assert_eq!(
        built("list_available_php", json!({})),
        Request::AvailablePhp
    );
    assert_eq!(built("list_services", json!({})), Request::ListServices);
    assert_eq!(built("list_mails", json!({})), Request::ListMails);
    assert_eq!(built("dumps_status", json!({})), Request::DumpsStatus);
    assert_eq!(built("status", json!({})), Request::Status);
    assert_eq!(built("diagnose", json!({})), Request::Diagnose);
    assert_eq!(
        built("get_mail", json!({ "id": "000007" })),
        Request::GetMail {
            id: "000007".to_owned()
        }
    );
    assert_eq!(
        built("list_databases", json!({ "service": "mysql" })),
        Request::ListDatabases {
            service: "mysql".to_owned()
        }
    );
}

#[test]
fn site_tools_map_to_their_requests() {
    assert_eq!(
        built("link_site", json!({ "name": "app", "path": "/srv/app" })),
        Request::Link {
            name: "app".to_owned(),
            path: PathBuf::from("/srv/app"),
        }
    );
    assert_eq!(
        built("park_directory", json!({ "path": "/srv/sites" })),
        Request::Park {
            path: PathBuf::from("/srv/sites"),
        }
    );
    assert_eq!(
        built("set_site_php", json!({ "name": "app", "version": "8.3" })),
        Request::SetPhp {
            name: "app".to_owned(),
            version: PhpVersion::new(8, 3),
        }
    );
    assert_eq!(
        built("set_site_secure", json!({ "name": "app", "secure": false })),
        Request::SetSecure {
            name: "app".to_owned(),
            secure: false,
        }
    );
    assert_eq!(
        built(
            "add_domain",
            json!({ "name": "app", "domain": "api.app.test" })
        ),
        Request::AddDomain {
            name: "app".to_owned(),
            domain: "api.app.test".to_owned(),
        }
    );
}

#[test]
fn proxy_tools_map_to_their_requests() {
    assert_eq!(
        built(
            "add_proxy",
            json!({ "name": "rev", "url": "http://127.0.0.1:8080" })
        ),
        Request::AddProxy {
            name: "rev".to_owned(),
            url: "http://127.0.0.1:8080".to_owned(),
        }
    );
    assert_eq!(
        built(
            "add_proxy_rule",
            json!({ "site": "app", "prefix": "/reverb", "url": "http://127.0.0.1:8080" })
        ),
        Request::AddProxyRule {
            site: "app".to_owned(),
            prefix: "/reverb".to_owned(),
            url: "http://127.0.0.1:8080".to_owned(),
        }
    );
    assert_eq!(
        built(
            "remove_proxy_rule",
            json!({ "site": "app", "prefix": "/reverb" })
        ),
        Request::RemoveProxyRule {
            site: "app".to_owned(),
            prefix: "/reverb".to_owned(),
        }
    );
}

/// Reject one proxy upstream and return the error message, failing if the call
/// was accepted. Asserts on the *message*, not just `-32602`: `UpstreamTarget`
/// rejects malformed URLs with the same code, so a code-only assertion would
/// pass even if the loopback gate were removed entirely.
fn rejected_upstream(tool: &str, args: Value) -> String {
    let line = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    })
    .to_string();
    let Outgoing::Reply(reply) = ready().handle_line(&line) else {
        panic!("{tool} accepted upstream {args}");
    };
    let v: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(
        v.pointer("/error/code"),
        Some(&json!(-32602)),
        "{tool} with {args}"
    );
    v.pointer("/error/message")
        .and_then(Value::as_str)
        .expect("error message")
        .to_owned()
}

/// Proxy upstreams are the only tool arguments whose effect leaves the machine:
/// a rule pointing a trusted `.test` origin at a remote host exfiltrates the
/// developer's cookies and tokens there. An agent can be talked into that by
/// injected content, so the catalog refuses non-local upstreams even though the
/// CLI allows them.
///
/// The `localhost`-lookalike hosts are the point of the exercise. They are why
/// the gate matches the host exactly instead of by substring or suffix: every
/// one of them is a host an attacker controls, and every one of them contains a
/// string that looks reassuring.
#[test]
fn proxy_upstreams_must_be_local() {
    let lookalikes = [
        "http://localhost.attacker.example",
        "http://127.0.0.1.attacker.example",
        "http://notlocalhost",
        "http://localhost.example.com:8080",
        "https://attacker.example/#localhost",
    ];
    for url in lookalikes {
        let message = rejected_upstream("add_proxy", json!({ "name": "rev", "url": url }));
        assert!(
            message.contains("not a loopback address") || message.contains("is invalid"),
            "{url} must not be mistaken for local: {message}"
        );
    }

    for (tool, args) in [
        (
            "add_proxy",
            json!({ "name": "rev", "url": "https://attacker.example" }),
        ),
        (
            "add_proxy_rule",
            json!({ "site": "app", "prefix": "/api", "url": "http://attacker.example:8080" }),
        ),
        (
            "add_proxy_rule",
            json!({ "site": "app", "prefix": "/api", "url": "http://10.0.0.5:8080" }),
        ),
        (
            "add_proxy",
            json!({ "name": "rev", "url": "http://[2001:db8::1]:80" }),
        ),
    ] {
        let message = rejected_upstream(tool, args);
        assert!(
            message.contains("not a loopback address"),
            "the gate, not the URL parser, should refuse this: {message}"
        );
    }
}

/// Spellings that do resolve locally but that the gate refuses anyway, because
/// it will not out-guess the resolver. Pinned so the safe-fail direction is a
/// decision rather than an accident: each must be refused, and the message must
/// point at the spelling that works.
#[test]
fn ambiguous_local_spellings_fail_safe() {
    for url in [
        "http://0.0.0.0:8000",
        "http://[::ffff:127.0.0.1]:80",
        "http://127.1",
        "http://2130706433",
    ] {
        let message = rejected_upstream("add_proxy", json!({ "name": "rev", "url": url }));
        assert!(
            message.contains("127.0.0.1"),
            "{url} was refused without naming the spelling that works: {message}"
        );
    }
}

#[test]
fn local_proxy_upstreams_are_accepted() {
    for url in [
        "http://127.0.0.1:8080",
        "http://localhost:3000",
        "https://127.0.0.1",
        "http://[::1]:9000",
    ] {
        assert_eq!(
            built("add_proxy", json!({ "name": "rev", "url": url })),
            Request::AddProxy {
                name: "rev".to_owned(),
                url: url.to_owned(),
            },
            "{url} should be accepted"
        );
    }
}

#[test]
fn php_tools_map_to_their_requests() {
    assert_eq!(
        built("install_php", json!({ "version": "8.5" })),
        Request::InstallPhpStreamed {
            version: PhpVersion::new(8, 5),
            confirm_legacy: false,
        },
        "install_php uses the streamed (job) variant so the call returns at once"
    );
    assert_eq!(
        built("set_default_php", json!({ "version": "8.4" })),
        Request::SetDefaultPhp {
            version: PhpVersion::new(8, 4),
        }
    );
    let mut settings = BTreeMap::new();
    settings.insert("memory_limit".to_owned(), "512M".to_owned());
    assert_eq!(
        built(
            "set_php_setting",
            json!({ "name": "memory_limit", "value": "512M" })
        ),
        Request::SetPhpSettings { settings }
    );
}

#[test]
fn paging_tools_default_their_cursors_to_zero() {
    assert_eq!(
        built("list_dumps", json!({})),
        Request::ListDumps { since_id: 0 }
    );
    assert_eq!(
        built("list_dumps", json!({ "since_id": 42 })),
        Request::ListDumps { since_id: 42 }
    );
    assert_eq!(
        built("job_status", json!({ "job_id": "j1" })),
        Request::JobStatus {
            job_id: "j1".to_owned(),
            cursor: 0,
        }
    );
    assert_eq!(
        built("job_status", json!({ "job_id": "j1", "cursor": 12 })),
        Request::JobStatus {
            job_id: "j1".to_owned(),
            cursor: 12,
        }
    );
}

#[test]
fn create_site_defaults_match_the_gui_wizard() {
    let request = built(
        "create_site",
        json!({ "name": "app", "parent_dir": "/srv", "php": "8.4" }),
    );
    let Request::CreateSite { spec } = request else {
        panic!("expected CreateSite");
    };
    assert_eq!(spec.name, "app");
    assert_eq!(spec.parent_dir, PathBuf::from("/srv"));
    assert_eq!(spec.php, PhpVersion::new(8, 4));
    assert!(spec.secure, "sites default to HTTPS");
    let yerd_ipc::Framework::Laravel { options } = spec.framework else {
        panic!("v1 creates Laravel sites");
    };
    assert_eq!(options.starter_kit, yerd_ipc::StarterKit::None);
    assert_eq!(options.testing, yerd_ipc::Testing::Pest);
    assert_eq!(options.database, yerd_ipc::Database::Sqlite);
    assert_eq!(options.js, yerd_ipc::JsRuntime::Npm);
    assert_eq!(options.auth, yerd_ipc::AuthProvider::Laravel);
    assert!(!options.git);
    assert!(!options.boost);
    assert!(!options.teams);
    assert!(!options.livewire_class_components);
}

#[test]
fn create_site_options_are_mapped() {
    let request = built(
        "create_site",
        json!({
            "name": "app",
            "parent_dir": "/srv",
            "php": "8.4",
            "secure": false,
            "starter_kit": "livewire",
            "testing": "phpunit",
            "database": "pgsql",
            "js": "skip",
            "git": true,
        }),
    );
    let Request::CreateSite { spec } = request else {
        panic!("expected CreateSite");
    };
    assert!(!spec.secure);
    let yerd_ipc::Framework::Laravel { options } = spec.framework else {
        panic!("expected Laravel");
    };
    assert_eq!(options.starter_kit, yerd_ipc::StarterKit::Livewire);
    assert_eq!(options.testing, yerd_ipc::Testing::PhpUnit);
    assert_eq!(options.database, yerd_ipc::Database::Pgsql);
    assert_eq!(
        options.js,
        yerd_ipc::JsRuntime::Skip,
        "skip lets an agent create a site on a machine with no Node"
    );
    assert!(options.git);
}
