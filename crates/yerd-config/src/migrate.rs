//! Schema-versioning machinery: read the `version` key and walk forward
//! migration steps.

use toml::Value;

use crate::error::MigrationErrorReason;
use crate::ConfigError;

/// A forward migration step: `v_N → v_{N+1}`, applied in place to the
/// parsed [`toml::Value`]. The step is responsible for leaving the
/// `version` key set to `N + 1` on success.
///
/// Migrations need not produce a *valid* config - the parser unconditionally
/// runs wire-mirror deserialisation (per-field invariants via `yerd-core`)
/// and [`crate::Config::validate`] (cross-field invariants) after the final
/// migration step, so the validator is the gate.
pub(crate) type MigrationStep = fn(&mut Value) -> Result<(), ConfigError>;

/// Forward-migration steps, indexed so that **`STEPS[N]` walks `vN → v(N+1)`**.
/// This matches [`up`], which indexes `STEPS[current]` (== the version being
/// migrated *from*). Example: a v1 file is migrated by `STEPS[1]`. When
/// `CURRENT_VERSION == 16`, `STEPS = [v0→v1, …, v14→v15, v15→v16]`, length 16.
///
/// `STEPS[0]` (v0→v1) is only reachable via a hand-crafted `version = 0` file -
/// v0 was never written to disk - but it must exist so that `STEPS[1]` does.
pub(crate) const STEPS: &[MigrationStep] = &[
    migrate_v0_to_v1,
    migrate_v1_to_v2,
    migrate_v2_to_v3,
    migrate_v3_to_v4,
    migrate_v4_to_v5,
    migrate_v5_to_v6,
    migrate_v6_to_v7,
    migrate_v7_to_v8,
    migrate_v8_to_v9,
    migrate_v9_to_v10,
    migrate_v10_to_v11,
    migrate_v11_to_v12,
    migrate_v12_to_v13,
    migrate_v13_to_v14,
    migrate_v14_to_v15,
    migrate_v15_to_v16,
];

/// `v0 → v1`: bump the version. v0 predates any shipped config, so there is no
/// structural change to apply.
fn migrate_v0_to_v1(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 1)
}

/// `v1 → v2`: bump the version. v2 added the optional `web_subpath`
/// (`[[linked]]`) and `web_root` (`[[overrides]]`) keys, both of which default
/// when absent, so an in-place version bump is the entire migration.
fn migrate_v1_to_v2(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 2)
}

/// `v2 → v3`: the first *structural* migration. v2 stored enabled services as
/// `[services]\nenabled = ["redis", "mysql"]`; v3 stores per-service tables
/// `[services.redis]\nenabled = true`. Rewrite the `enabled` array (if present)
/// into those tables, then drop the `enabled` key. Anything else under
/// `[services]` is left untouched so a genuinely malformed table still fails the
/// (`deny_unknown_fields`) wire deserialisation that runs after migration.
fn migrate_v2_to_v3(value: &mut Value) -> Result<(), ConfigError> {
    if let Some(services) = value
        .as_table_mut()
        .and_then(|t| t.get_mut("services"))
        .and_then(Value::as_table_mut)
    {
        if let Some(Value::Array(enabled)) = services.remove("enabled") {
            for entry in enabled {
                if let Value::String(name) = entry {
                    let mut inst = toml::value::Table::new();
                    inst.insert("enabled".to_string(), Value::Boolean(true));
                    services.insert(name, Value::Table(inst));
                }
            }
        }
    }
    set_version(value, 3)
}

/// `v3 → v4`: bump the version. v4 added the optional `[mail]` section, which
/// defaults when absent, so an in-place version bump is the entire migration.
fn migrate_v3_to_v4(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 4)
}

/// `v4 → v5`: bump the version. v5 added the optional `[dumps]` table, which
/// defaults when absent, so an in-place version bump is the entire migration.
fn migrate_v4_to_v5(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 5)
}

/// `v5 → v6`: bump the version. v6 added the top-level `update_channel` scalar,
/// which defaults to `"stable"` when absent, so an in-place version bump is the
/// entire migration.
fn migrate_v5_to_v6(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 6)
}

/// `v6 → v7`: bump the version. v7 added the `[ports] fallback_http`/
/// `fallback_https` keys, which default to `8080`/`8443` when absent, so an
/// in-place version bump is the entire migration.
fn migrate_v6_to_v7(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 7)
}

/// `v7 → v8`: bump the version. v8 added the optional `[tunnel]` table, which
/// defaults (empty) when absent, so an in-place version bump is the entire
/// migration.
fn migrate_v7_to_v8(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 8)
}

/// `v8 → v9`: bump the version. v9 added the optional `[groups]` table, which
/// defaults (empty) when absent, so an in-place version bump is the entire
/// migration.
fn migrate_v8_to_v9(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 9)
}

/// `v9 → v10`: bump the version. v10 added the optional `[php.extensions]`
/// registry and the `wp_auto_login`/`wp_auto_login_user` keys (inside
/// `[[linked]]` and `[[overrides]]`), all of which default when absent, so an
/// in-place version bump is the entire migration.
fn migrate_v9_to_v10(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 10)
}

/// `v10 → v11`: bump the version. v11 added the optional `[domains]` table, which
/// defaults (empty) when absent, so an in-place version bump is the entire
/// migration.
fn migrate_v10_to_v11(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 11)
}

/// `v11 → v12`: bump the version. v12 added the top-level `symlink_protection`
/// scalar, which defaults to `true` when absent, so an in-place version bump is
/// the entire migration.
fn migrate_v11_to_v12(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 12)
}

/// `v12 → v13`: bump the version. v13 added the optional per-site
/// `front_controller` key (`[[linked]]` and `[[overrides]]`), which defaults to
/// auto when absent, so an in-place version bump is the entire migration.
fn migrate_v12_to_v13(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 13)
}

/// `v13 → v14`: bump the version. v14 added the optional `[[proxies]]` array and
/// `[proxy_rules]` table, both of which default to empty when absent, so an
/// in-place version bump is the entire migration.
fn migrate_v13_to_v14(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 14)
}

/// `v14 → v15`: the multi-instance services rework. v15 added the optional
/// per-instance `site` field and the `"{type}:{site}"` wire ids (both additive),
/// and made the `enabled` flag actually gate boot autostart. Before v15 the
/// daemon auto-started *every installed* engine regardless of `enabled`; to keep
/// a user's previously-installed engines (redis, the databases) starting with
/// Yerd across the upgrade, mark every existing single-instance engine
/// (colon-free key) `enabled = true`. Per-site entries don't exist yet at v14, so
/// none are affected.
fn migrate_v14_to_v15(value: &mut Value) -> Result<(), ConfigError> {
    if let Some(services) = value.get_mut("services").and_then(Value::as_table_mut) {
        for (key, inst) in services.iter_mut() {
            if !key.contains(':') {
                if let Some(table) = inst.as_table_mut() {
                    table.insert("enabled".to_string(), Value::Boolean(true));
                }
            }
        }
    }
    set_version(value, 15)
}

/// `v15 → v16`: bump the version. v16 added the top-level `mcp_enabled` scalar,
/// which defaults to `false` when absent, so an in-place version bump is the
/// entire migration.
fn migrate_v15_to_v16(value: &mut Value) -> Result<(), ConfigError> {
    set_version(value, 16)
}

/// Set the top-level `version` key, erroring if the root is not a table.
fn set_version(value: &mut Value, n: i64) -> Result<(), ConfigError> {
    let table = value.as_table_mut().ok_or(ConfigError::Migration {
        reason: MigrationErrorReason::MissingVersion,
    })?;
    table.insert("version".to_string(), Value::Integer(n));
    Ok(())
}

/// Reads the top-level `version` key.
///
/// TOML's grammar guarantees a table root, so the non-table branch is
/// unreachable through [`toml::from_str`]. It still exists as a defensive
/// `as_table()` check; tests in `mod tests` construct a non-table
/// [`Value`] directly to exercise it.
pub(crate) fn read_version(value: &Value) -> Result<u32, ConfigError> {
    let table = value.as_table().ok_or(ConfigError::Migration {
        reason: MigrationErrorReason::MissingVersion,
    })?;
    let v = table.get("version").ok_or(ConfigError::Migration {
        reason: MigrationErrorReason::MissingVersion,
    })?;
    let n = v.as_integer().ok_or(ConfigError::Migration {
        reason: MigrationErrorReason::NonIntegerVersion,
    })?;
    u32::try_from(n).map_err(|_| ConfigError::Migration {
        reason: MigrationErrorReason::NonIntegerVersion,
    })
}

/// Walks [`STEPS`] from `found` up to [`crate::CURRENT_VERSION`].
///
/// Caller has already verified `found < CURRENT_VERSION`.
pub(crate) fn up(value: &mut Value, found: u32) -> Result<(), ConfigError> {
    let mut current = found;
    while current < crate::CURRENT_VERSION {
        let idx = usize::try_from(current).map_err(|_| ConfigError::Migration {
            reason: MigrationErrorReason::MissingStep { from: current },
        })?;
        let step = STEPS.get(idx).ok_or(ConfigError::Migration {
            reason: MigrationErrorReason::MissingStep { from: current },
        })?;
        step(value)?;
        current = current.checked_add(1).ok_or(ConfigError::Migration {
            reason: MigrationErrorReason::MissingStep { from: current },
        })?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use toml::Value;

    use super::*;

    #[test]
    fn steps_cover_every_version_below_current() {
        assert_eq!(STEPS.len(), crate::CURRENT_VERSION as usize);
    }

    #[test]
    fn current_version_pinned() {
        assert_eq!(crate::CURRENT_VERSION, 16);
    }

    #[test]
    fn v13_to_v14_is_a_bare_version_bump() {
        let mut v: Value = toml::from_str("version = 13\n").unwrap();
        migrate_v13_to_v14(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 14);
    }

    #[test]
    fn v15_to_v16_is_a_bare_version_bump() {
        let mut v: Value = toml::from_str("version = 15\n").unwrap();
        migrate_v15_to_v16(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 16);
    }

    #[test]
    fn v3_to_v4_is_a_bare_version_bump() {
        let mut v: Value = toml::from_str("version = 3\n").unwrap();
        migrate_v3_to_v4(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 4);
    }

    #[test]
    fn v4_to_v5_is_a_bare_version_bump() {
        let mut v: Value = toml::from_str("version = 4\n").unwrap();
        migrate_v4_to_v5(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 5);
    }

    #[test]
    fn v5_to_v6_is_a_bare_version_bump() {
        let mut v: Value = toml::from_str("version = 5\n").unwrap();
        migrate_v5_to_v6(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 6);
    }

    #[test]
    fn v6_to_v7_is_a_bare_version_bump() {
        let mut v: Value = toml::from_str("version = 6\n").unwrap();
        migrate_v6_to_v7(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 7);
    }

    #[test]
    fn v8_to_v9_is_a_bare_version_bump() {
        let mut v: Value = toml::from_str("version = 8\n").unwrap();
        migrate_v8_to_v9(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 9);
    }

    #[test]
    fn v9_to_v10_is_a_bare_version_bump() {
        let mut v: Value = toml::from_str("version = 9\n").unwrap();
        migrate_v9_to_v10(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 10);
    }

    #[test]
    fn v10_to_v11_is_a_bare_version_bump() {
        let mut v: Value = toml::from_str("version = 10\n").unwrap();
        migrate_v10_to_v11(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 11);
    }

    #[test]
    fn v11_to_v12_is_a_bare_version_bump() {
        let mut v: Value = toml::from_str("version = 11\n").unwrap();
        migrate_v11_to_v12(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 12);
    }

    #[test]
    fn v12_to_v13_is_a_bare_version_bump() {
        let mut v: Value = toml::from_str("version = 12\n").unwrap();
        migrate_v12_to_v13(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 13);
    }

    #[test]
    fn v14_to_v15_bumps_version_with_no_services_section() {
        let mut v: Value = toml::from_str("version = 14\n").unwrap();
        migrate_v14_to_v15(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 15);
    }

    #[test]
    fn v14_to_v15_enables_existing_single_instance_engines() {
        let mut v: Value = toml::from_str(
            "version = 14\n[services.redis]\nenabled = false\n[services.mysql]\nversion = \"8.4\"\n",
        )
        .unwrap();
        migrate_v14_to_v15(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 15);
        // A stopped-at-upgrade engine and an engine with no explicit flag both
        // become enabled, so they keep starting with Yerd after the upgrade.
        assert_eq!(v["services"]["redis"]["enabled"].as_bool(), Some(true));
        assert_eq!(v["services"]["mysql"]["enabled"].as_bool(), Some(true));
    }

    #[test]
    fn v2_to_v3_rewrites_enabled_array_into_service_tables() {
        let mut v: Value =
            toml::from_str("version = 2\n[services]\nenabled = [\"redis\", \"mysql\"]\n").unwrap();
        migrate_v2_to_v3(&mut v).unwrap();
        assert_eq!(read_version(&v).unwrap(), 3);
        let services = v.get("services").and_then(Value::as_table).unwrap();
        assert!(
            services.get("enabled").is_none(),
            "enabled array must be removed"
        );
        for name in ["redis", "mysql"] {
            let inst = services.get(name).and_then(Value::as_table).unwrap();
            assert_eq!(inst.get("enabled"), Some(&Value::Boolean(true)));
        }
    }

    #[test]
    fn read_version_accepts_canonical() {
        let v: Value = toml::from_str("version = 1").unwrap();
        assert_eq!(read_version(&v).unwrap(), 1);
    }

    #[test]
    fn read_version_rejects_missing_key() {
        let v: Value = toml::from_str("tld = \"test\"").unwrap();
        match read_version(&v) {
            Err(ConfigError::Migration {
                reason: MigrationErrorReason::MissingVersion,
            }) => {}
            other => panic!("expected MissingVersion, got {other:?}"),
        }
    }

    #[test]
    fn read_version_rejects_non_integer() {
        let v: Value = toml::from_str("version = \"1\"").unwrap();
        match read_version(&v) {
            Err(ConfigError::Migration {
                reason: MigrationErrorReason::NonIntegerVersion,
            }) => {}
            other => panic!("expected NonIntegerVersion, got {other:?}"),
        }
    }

    #[test]
    fn read_version_rejects_negative() {
        let v: Value = toml::from_str("version = -1").unwrap();
        match read_version(&v) {
            Err(ConfigError::Migration {
                reason: MigrationErrorReason::NonIntegerVersion,
            }) => {}
            other => panic!("expected NonIntegerVersion for negative, got {other:?}"),
        }
    }

    /// Pins the defensive non-table branch via a hand-constructed
    /// `Value::Integer(42)` - unreachable through `toml::from_str`
    /// (TOML's grammar guarantees a table root).
    #[test]
    fn read_version_rejects_non_table_root_via_constructed_value() {
        let v = Value::Integer(42);
        match read_version(&v) {
            Err(ConfigError::Migration {
                reason: MigrationErrorReason::MissingVersion,
            }) => {}
            other => panic!("expected MissingVersion via non-table branch, got {other:?}"),
        }
    }

    #[test]
    fn up_migrates_v1_to_current_via_steps_index_one() {
        let mut v: Value = toml::from_str("version = 1").unwrap();
        up(&mut v, 1).unwrap();
        assert_eq!(read_version(&v).unwrap(), crate::CURRENT_VERSION);
    }

    #[test]
    fn up_walks_v0_all_the_way_to_current() {
        let mut v: Value = toml::from_str("version = 0").unwrap();
        up(&mut v, 0).unwrap();
        assert_eq!(read_version(&v).unwrap(), crate::CURRENT_VERSION);
    }
}
