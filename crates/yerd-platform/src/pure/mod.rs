//! Pure, in-memory decision helpers used by the OS impls.
//!
//! Every function in this module is sync, runtime-free, and free of I/O,
//! clock reads, and environment lookups. Each submodule is unit-tested
//! table-style.

pub mod cert_identity;
pub mod dns_probe;
pub mod firefox;
pub mod helper_result;
pub mod ide_spec;
pub mod networkmanager_dnsmasq;
pub mod nrpt;
pub mod nss;
pub mod opener_spec;
pub mod pem_match;
pub mod pf_anchor;
pub mod port_plan;
pub mod proc_metrics;
pub mod ps_metrics;
pub mod resolv_conf;
pub mod resolved_drop_in;
pub mod resolver_file;
pub mod shell_profile;
pub mod system_roots;
pub mod terminal_spec;
pub mod win_path_env;
pub mod win_pipe;
pub mod win_port_owner;
pub mod win_shim;
pub mod win_terminal;
pub mod win_token;
