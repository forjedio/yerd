//! Pure, in-memory decision helpers used by the OS impls.
//!
//! Every function in this module is sync, runtime-free, and free of I/O,
//! clock reads, and environment lookups. Each submodule is unit-tested
//! table-style.

pub mod cert_identity;
pub mod dns_probe;
pub mod firefox;
pub mod networkmanager_dnsmasq;
pub mod nss;
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
