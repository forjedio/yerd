//! Render and parse round-trip of a `.yerd.toml`, pinned to exact bytes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use yerd_config::ProjectFile;

const EXAMPLE: &str = r#"# Yerd project file. Commit it, then teammates run `yerd init`.
# Reference: https://yerd.app/reference/project-file

name = "acme"
php = "8.4"
secure = true
node = "22"
domains = ["*.acme", "admin.acme"]
extensions = ["imagick", "redis"]

[headers]
Access-Control-Allow-Origin = "*"

[proxy_rules]
"/app" = "http://127.0.0.1:3000"

[services]
mysql = "8.4"
redis = "*"
"#;

#[test]
fn example_renders_byte_for_byte() {
    let file = ProjectFile::from_toml(EXAMPLE).unwrap();
    assert_eq!(file.to_toml(), EXAMPLE);
}

#[test]
fn render_parse_round_trip_is_stable() {
    let inputs = [
        EXAMPLE,
        "",
        "secure = false",
        "php = \"8.3\"\n[services]\npostgres = \"17\"",
        "[headers]\nX-B = \"2\"\nx-a = \"1\"",
        "[proxy_rules]\n\"/ws\" = \"https://[::1]:6001\"\n\"/\" = \"http://localhost\"",
    ];
    for input in inputs {
        let file = ProjectFile::from_toml(input).unwrap();
        let rendered = file.to_toml();
        let reparsed = ProjectFile::from_toml(&rendered).unwrap();
        assert_eq!(reparsed, file, "{input:?}");
        assert_eq!(reparsed.to_toml(), rendered, "{input:?}");
    }
}

#[test]
fn headers_render_sorted_ignoring_case() {
    let file = ProjectFile::from_toml("[headers]\nX-B = \"2\"\nx-a = \"1\"").unwrap();
    assert!(file
        .to_toml()
        .ends_with("[headers]\nx-a = \"1\"\nX-B = \"2\"\n"));
}
