//! Schema-versioning machinery: read the `version` key and walk forward
//! migration steps.
//!
//! In v0 [`STEPS`] is empty and [`crate::CURRENT_VERSION`] is `1`. The
//! scaffold exists so future migrations slot in without restructuring the
//! parse path.

use toml::Value;

use crate::error::MigrationErrorReason;
use crate::ConfigError;

/// A forward migration step: `v_N → v_{N+1}`, applied in place to the
/// parsed [`toml::Value`]. The step is responsible for leaving the
/// `version` key set to `N + 1` on success.
///
/// Migrations need not produce a *valid* config — the parser unconditionally
/// runs wire-mirror deserialisation (per-field invariants via `yerd-core`)
/// and [`crate::Config::validate`] (cross-field invariants) after the final
/// migration step, so the validator is the gate.
pub(crate) type MigrationStep = fn(&mut Value) -> Result<(), ConfigError>;

/// Forward-migration steps.
///
/// `STEPS[i]` walks `v(i + 1) → v(i + 2)`. Example: when
/// `CURRENT_VERSION == 3`, `STEPS = [v1→v2, v2→v3]`, length 2. In v0
/// `CURRENT_VERSION == 1` and `STEPS` is empty.
pub(crate) const STEPS: &[MigrationStep] = &[];

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
    fn steps_empty_in_v0() {
        assert!(STEPS.is_empty());
    }

    #[test]
    fn current_version_pinned_to_one() {
        assert_eq!(crate::CURRENT_VERSION, 1);
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
    /// `Value::Integer(42)` — unreachable through `toml::from_str`
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

    /// Exercises the migration loop body via a local fake-step slice that
    /// mirrors `up()`'s body but uses a private `STEPS_FAKE` array. We do
    /// not modify the real `STEPS` because tests must not perturb runtime
    /// state.
    #[test]
    fn up_walks_with_test_only_step() {
        fn bump_v0_to_v1(value: &mut Value) -> Result<(), ConfigError> {
            let table = value.as_table_mut().ok_or(ConfigError::Migration {
                reason: MigrationErrorReason::MissingVersion,
            })?;
            table.insert("version".to_string(), Value::Integer(1));
            Ok(())
        }
        let steps_fake: &[MigrationStep] = &[bump_v0_to_v1];

        let mut v: Value = toml::from_str("version = 0").unwrap();
        let mut current = 0u32;
        let target = 1u32;
        while current < target {
            let idx = usize::try_from(current).unwrap();
            let step = steps_fake.get(idx).expect("test step present");
            step(&mut v).unwrap();
            current = current.checked_add(1).unwrap();
        }
        assert_eq!(read_version(&v).unwrap(), 1);
    }

    #[test]
    fn up_returns_missing_step_when_steps_empty_and_found_below_current() {
        // `up()` requires `found < CURRENT_VERSION` (== 1). With STEPS
        // empty, `STEPS.get(0)` returns None → MissingStep.
        let mut v: Value = toml::from_str("version = 0").unwrap();
        match up(&mut v, 0) {
            Err(ConfigError::Migration {
                reason: MigrationErrorReason::MissingStep { from: 0 },
            }) => {}
            other => panic!("expected MissingStep {{ from: 0 }}, got {other:?}"),
        }
    }
}
