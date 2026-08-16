//! Pure parse of `netstat` and `tasklist` output for "who holds this UDP port".
//!
//! Compiled on every OS so Linux/macOS CI table-tests it too. No I/O: the
//! caller spawns `netstat.exe` / `tasklist.exe` by absolute path and hands the
//! captured stdout here.
//!
//! Both tools print a localized banner and, for `tasklist`, a localized
//! "no tasks" INFO line. Neither parser matches on prose: the `netstat` reader
//! keys off the `UDP` protocol literal and the exact local-address column, and
//! the `tasklist` reader keys off the leading quote of a CSV data row, which the
//! INFO line never has. That is the same locale-dodge
//! `yerd_service_ctl`'s `tasklist_lists` uses.

/// The PID owning the UDP socket bound to `addr`, from `netstat -a -n -o -p UDP`.
///
/// `addr` is matched against the "Local Address" column verbatim, so the caller
/// decides whether it wants the loopback form (`127.0.0.1:53`) or the wildcard
/// (`0.0.0.0:53`). UDP rows carry no State column, so the PID is the last field.
/// Rows that are not UDP, do not name `addr`, or are truncated are skipped.
#[must_use]
pub fn udp_owning_pid(netstat_stdout: &str, addr: &str) -> Option<u32> {
    netstat_stdout.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        if !fields.next()?.eq_ignore_ascii_case("UDP") {
            return None;
        }
        if fields.next()? != addr {
            return None;
        }
        fields.last()?.parse().ok()
    })
}

/// The image name from the first data row of `tasklist /FO CSV /NH` output.
///
/// A data row starts with the quoted image name (`"yerdd.exe",...`); the
/// localized "no tasks" INFO line does not, so it falls through to `None`.
/// A quoted-but-empty first field is treated as no answer.
#[must_use]
pub fn image_name(tasklist_csv: &str) -> Option<String> {
    tasklist_csv.lines().find_map(|line| {
        let (name, _) = line.trim_start().strip_prefix('"')?.split_once('"')?;
        (!name.is_empty()).then(|| name.to_owned())
    })
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

    /// Captured verbatim from `netstat -a -n -o -p UDP` on Windows 11.
    const NETSTAT: &str = "\r
Active Connections\r
\r
  Proto  Local Address          Foreign Address        State           PID\r
  UDP    0.0.0.0:5050           *:*                                    4932\r
  UDP    0.0.0.0:5353           *:*                                    18528\r
  UDP    0.0.0.0:58030          160.79.104.10:443                      15168\r
  UDP    127.0.0.1:53           *:*                                    11616\r
  UDP    127.0.0.1:1900         *:*                                    9932\r
  UDP    192.168.0.91:137       *:*                                    4\r
";

    #[test]
    fn udp_owning_pid_table() {
        let cases: &[(&str, Option<u32>)] = &[
            ("127.0.0.1:53", Some(11616)),
            ("0.0.0.0:5050", Some(4932)),
            ("192.168.0.91:137", Some(4)),
            ("0.0.0.0:53", None),
            ("127.0.0.1:5353", None),
            ("", None),
        ];
        for (addr, want) in cases {
            assert_eq!(udp_owning_pid(NETSTAT, addr), *want, "addr {addr:?}");
        }
    }

    #[test]
    fn udp_owning_pid_ignores_non_udp_and_malformed_rows() {
        let mixed = "  TCP    127.0.0.1:53           0.0.0.0:0              LISTENING       777\n\
                       UDP    127.0.0.1:53\n\
                       UDP    127.0.0.1:53           *:*                                    not-a-pid\n";
        assert_eq!(udp_owning_pid(mixed, "127.0.0.1:53"), None);
        assert_eq!(udp_owning_pid("", "127.0.0.1:53"), None);
        assert_eq!(
            udp_owning_pid("garbage without columns", "127.0.0.1:53"),
            None
        );
    }

    #[test]
    fn udp_owning_pid_takes_the_first_match() {
        let dupes =
            "  UDP    0.0.0.0:5353           *:*                                    18528\n\
                       UDP    0.0.0.0:5353           *:*                                    2460\n";
        assert_eq!(udp_owning_pid(dupes, "0.0.0.0:5353"), Some(18528));
    }

    #[test]
    fn image_name_table() {
        let cases: &[(&str, Option<&str>)] = &[
            (
                "\"yerdd.exe\",\"11616\",\"Console\",\"1\",\"9,840 K\"\r\n",
                Some("yerdd.exe"),
            ),
            (
                "INFO: No tasks are running which match the specified criteria.\r\n",
                None,
            ),
            ("", None),
            ("\"\",\"1\"\r\n", None),
            ("\"unterminated\r\n", None),
            (
                "INFO: banner\r\n\"svchost.exe\",\"4\",\"Services\",\"0\",\"12 K\"\r\n",
                Some("svchost.exe"),
            ),
        ];
        for (csv, want) in cases {
            assert_eq!(image_name(csv).as_deref(), *want, "csv {csv:?}");
        }
    }
}
