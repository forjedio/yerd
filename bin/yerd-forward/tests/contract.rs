//! The forwarder's command line is written by `yerd-helper` from
//! `yerd_platform::pure::launchd_ports`; these tests keep the two in step.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::ffi::OsString;
use std::path::Path;

use yerd_forward::{parse_args, Command, Scope, SCOPE_FLAG, TO_FLAG};
use yerd_platform::pure::launchd_ports::{
    self, compose_forwarder_plist, FORWARDER_SCOPE_FLAG, FORWARDER_TO_FLAG,
};

#[test]
fn flags_match() {
    assert_eq!(TO_FLAG, FORWARDER_TO_FLAG);
    assert_eq!(SCOPE_FLAG, FORWARDER_SCOPE_FLAG);
}

#[test]
fn composed_plist_argv_parses() {
    let pairs = [
        (launchd_ports::Scope::Loopback, Scope::Loopback),
        (launchd_ports::Scope::Lan, Scope::Lan),
    ];
    for (platform_scope, forward_scope) in pairs {
        assert_eq!(platform_scope.arg(), forward_scope.as_arg());
        let plist = compose_forwarder_plist(
            platform_scope,
            "dev.yerd.test",
            Path::new("/opt/yerd-forward"),
            "someone",
            443,
            8443,
        );
        let args_start = plist.find("<array>").unwrap();
        let args_end = plist.find("</array>").unwrap();
        let argv: Vec<OsString> = plist[args_start..args_end]
            .split("<string>")
            .skip(2)
            .filter_map(|c| c.split_once("</string>").map(|(v, _)| OsString::from(v)))
            .collect();
        assert_eq!(
            parse_args(&argv),
            Ok(Command::Forward {
                scope: forward_scope,
                to: 8443
            })
        );
    }
}
